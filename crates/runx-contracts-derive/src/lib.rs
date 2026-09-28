//! `#[derive(RunxSchema)]`: emit a wire-conformant JSON Schema document from a
//! contract type's definition and its serde attributes. Part of Phase 1 of
//! `rust-contract-pipeline-inversion`.
//!
//! Supported today: named-field structs (honoring `#[serde(rename)]`,
//! `#[serde(rename_all)]`, `#[serde(skip)]`, `Option<T>` /
//! `#[serde(skip_serializing_if)]` / `#[serde(default)]` optionality (an
//! `Option<T>` without any omittability is required-but-nullable),
//! `#[serde(flatten)]` (the flattened type's properties merge into the parent;
//! a flattened map opens the parent to additional properties), and
//! `#[serde(deny_unknown_fields)]`), unit-only enums (rendered
//! as `type: string` with `enum`), and data-carrying enums under serde's default
//! (externally-tagged), internally-tagged (`#[serde(tag = "...")]`), and
//! `#[serde(untagged)]` representations (each rendered as an `anyOf` of variant
//! subschemas). Multi-field tuple variants are not modeled.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, Fields, GenericArgument, Lit, PathArguments, Type, parse_macro_input,
};

#[proc_macro_derive(RunxSchema, attributes(runx_schema))]
pub fn derive_runx_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let identity = runx_identity(&input.attrs)?;
    let identity_expr = match identity {
        Some(Identity::Runx { logical, url }) => {
            let url_expr = match url {
                Some(url) => quote! { ::core::option::Option::Some(#url) },
                None => quote! { ::core::option::Option::None },
            };
            quote! {
                ::core::option::Option::Some(
                    ::runx_contracts::schema::Identity::Runx {
                        logical: #logical,
                        url: #url_expr,
                    },
                )
            }
        }
        Some(Identity::BareId { url }) => quote! {
            ::core::option::Option::Some(
                ::runx_contracts::schema::Identity::BareId { url: #url },
            )
        },
        None => quote! { ::core::option::Option::None },
    };

    let body = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(_) => struct_body(input, data, &identity_expr)?,
            // A single-field tuple struct (commonly `#[serde(transparent)]`)
            // wrapping a `BTreeMap<String, T>` is a map-rooted document: it
            // emits the committed open-map shape (`patternProperties` over the
            // value type's schema), carrying any top-level identity.
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                map_newtype_body(ident, &fields.unnamed[0].ty, &identity_expr)?
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "RunxSchema supports only named-field structs or single-field map newtypes",
                ));
            }
        },
        Data::Enum(data) => enum_body(input, data, &identity_expr)?,
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "RunxSchema cannot be derived for unions",
            ));
        }
    };

    Ok(quote! {
        impl ::runx_contracts::schema::RunxSchema for #ident {
            fn json_schema() -> ::serde_json::Value {
                #body
            }
        }
    })
}

fn struct_body(
    input: &DeriveInput,
    data: &syn::DataStruct,
    identity_expr: &proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let rename_all = serde_rename_all(&input.attrs)?;
    let deny_unknown = serde_deny_unknown_fields(&input.attrs);
    // A container-level `#[serde(default)]` fills any omitted field, so every
    // field is optional regardless of its own attributes.
    let container_default = serde_default(&input.attrs);

    let (properties, flattened) =
        struct_field_properties(&data.fields, rename_all.as_deref(), container_default)?;

    Ok(object_body(
        &properties,
        &flattened,
        deny_unknown,
        identity_expr,
    ))
}

/// Emit the open-map document for a single-field map newtype. The wrapped field
/// must be a `BTreeMap<String, T>`; the document is `object_map_schema` over
/// `T`'s schema, carrying any top-level identity.
fn map_newtype_body(
    ident: &syn::Ident,
    ty: &Type,
    identity_expr: &proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let Some(value_ty) = map_value_type(ty) else {
        return Err(syn::Error::new_spanned(
            ident,
            "RunxSchema map newtype must wrap a `BTreeMap<String, T>`",
        ));
    };
    Ok(quote! {
        ::runx_contracts::schema::object_map_schema(
            <#value_ty as ::runx_contracts::schema::RunxSchema>::json_schema(),
            #identity_expr,
        )
    })
}

/// Extract `T` from a `BTreeMap<String, T>` type, returning `None` for any
/// other shape. Contract map roots are intentionally deterministic and
/// string-keyed; a `HashMap` or non-string key would make the emitted open-map
/// schema lie about the wire domain.
fn map_value_type(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "BTreeMap" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let mut type_args = args.args.iter().filter_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner.clone()),
        _ => None,
    });
    let key = type_args.next()?;
    if !is_string_type(&key) {
        return None;
    }
    type_args.next()
}

fn is_string_type(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "String")
}

/// Build the object-schema constructor call for a set of normal properties and
/// flattened sub-schemas. When there are no flattened fields the simpler
/// [`object_schema`] form is emitted; otherwise the flatten-merging form is
/// used.
fn object_body(
    properties: &[proc_macro2::TokenStream],
    flattened: &[proc_macro2::TokenStream],
    deny_unknown: bool,
    identity_expr: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    if flattened.is_empty() {
        quote! {
            ::runx_contracts::schema::object_schema(
                ::std::vec![#(#properties),*],
                #deny_unknown,
                #identity_expr,
            )
        }
    } else {
        quote! {
            ::runx_contracts::schema::object_schema_with_flatten(
                ::std::vec![#(#properties),*],
                ::std::vec![#(#flattened),*],
                #deny_unknown,
                #identity_expr,
            )
        }
    }
}

/// Build the `Property` constructors for a set of named fields, honoring
/// `rename`/`rename_all`, `skip`, and optionality. A field is NOT required when
/// it is `Option<...>`, carries a field-level `#[serde(default)]`, or the
/// container carries `#[serde(default)]`.
///
/// Returns `(properties, flattened)`: normal field `Property` constructors and,
/// separately, the emitted schemas of any `#[serde(flatten)]` fields (whose
/// inner properties merge into the parent at runtime).
fn struct_field_properties(
    fields: &Fields,
    rename_all: Option<&str>,
    container_default: bool,
) -> syn::Result<(Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>)> {
    let mut properties = Vec::new();
    let mut flattened = Vec::new();
    for field in fields {
        let Some(ident) = field.ident.as_ref() else {
            continue;
        };
        if serde_skip(&field.attrs) {
            continue;
        }
        // A `#[serde(flatten)]` field merges its inner type's properties into
        // the parent object rather than nesting under a key.
        if serde_flag(&field.attrs, "flatten") {
            let ty = &field.ty;
            flattened.push(quote! {
                <#ty as ::runx_contracts::schema::RunxSchema>::json_schema()
            });
            continue;
        }
        let wire_name = match serde_rename(&field.attrs)? {
            Some(name) => name,
            None => apply_rename_all(&ident.to_string(), rename_all),
        };
        let (inner_ty, optional) = unwrap_option(&field.ty);
        let field_default = serde_default(&field.attrs);
        let has_skip = serde_skip_serializing_if(&field.attrs);
        // An `Option<T>` is OMITTABLE (not required, plain inner schema) when it
        // can leave the wire absent: it carries `skip_serializing_if`, a field
        // `#[serde(default)]`, or the container defaults. Otherwise an
        // `Option<T>` is REQUIRED-BUT-NULLABLE: it must appear, and its property
        // schema is the inner schema unioned with `null`.
        let omittable = optional && (has_skip || field_default || container_default);
        let required = !(omittable || field_default || container_default);
        let nullable = optional && !omittable;
        let inner_schema = quote! {
            <#inner_ty as ::runx_contracts::schema::RunxSchema>::json_schema()
        };
        let property_schema = if nullable {
            quote! { ::runx_contracts::schema::nullable(#inner_schema) }
        } else {
            inner_schema
        };
        properties.push(quote! {
            ::runx_contracts::schema::Property::new(
                #wire_name,
                #property_schema,
                #required,
            )
        });
    }
    Ok((properties, flattened))
}

fn enum_body(
    input: &DeriveInput,
    data: &syn::DataEnum,
    identity_expr: &proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let rename_all = serde_rename_all(&input.attrs)?;
    let rename_all_fields = serde_rename_all_fields(&input.attrs)?;
    let untagged = serde_untagged(&input.attrs);
    let internal_tag = serde_tag(&input.attrs)?;
    // A container-level `#[serde(deny_unknown_fields)]` closes every variant's
    // object under internal tagging (serde rejects unknown fields per variant).
    let container_deny = serde_deny_unknown_fields(&input.attrs);

    // The simple, fast path: an all-unit enum is a closed string enum, one
    // `enum` keyword over its wire names. A top-level identity (when present)
    // sits alongside it.
    let all_unit = data
        .variants
        .iter()
        .all(|variant| matches!(variant.fields, Fields::Unit));
    if all_unit && internal_tag.is_none() && !untagged {
        let mut names = Vec::new();
        for variant in &data.variants {
            let wire_name = match serde_rename(&variant.attrs)? {
                Some(name) => name,
                None => apply_rename_all(&variant.ident.to_string(), rename_all.as_deref()),
            };
            names.push(wire_name);
        }
        return Ok(quote! {
            ::runx_contracts::schema::string_enum(
                &[#(#names),*],
                #identity_expr,
            )
        });
    }

    // Data-carrying enums render as an `anyOf` of per-variant subschemas. The
    // subschema shape depends on the serde representation.
    let mut variant_schemas = Vec::new();
    for variant in &data.variants {
        let wire_name = match serde_rename(&variant.attrs)? {
            Some(name) => name,
            None => apply_rename_all(&variant.ident.to_string(), rename_all.as_deref()),
        };
        let schema = if untagged {
            untagged_variant_schema(variant, rename_all_fields.as_deref())?
        } else if let Some(tag) = &internal_tag {
            internally_tagged_variant_schema(
                variant,
                &wire_name,
                tag,
                rename_all_fields.as_deref(),
                container_deny,
            )?
        } else {
            externally_tagged_variant_schema(variant, &wire_name, rename_all_fields.as_deref())?
        };
        variant_schemas.push(schema);
    }

    Ok(quote! {
        ::runx_contracts::schema::any_of_with_identity(
            ::std::vec![#(#variant_schemas),*],
            #identity_expr,
        )
    })
}

/// The payload schema for a variant under `#[serde(untagged)]`: the inner type's
/// schema for newtype variants, or an inlined object for struct variants. Unit
/// variants under untagged are unusual; we emit a closed const for them.
fn untagged_variant_schema(
    variant: &syn::Variant,
    rename_all_fields: Option<&str>,
) -> syn::Result<proc_macro2::TokenStream> {
    match &variant.fields {
        Fields::Unit => {
            let wire_name = apply_rename_all(&variant.ident.to_string(), None);
            Ok(quote! { ::runx_contracts::schema::const_string(#wire_name) })
        }
        Fields::Unnamed(fields) => newtype_inner_schema(variant, fields),
        Fields::Named(_) => {
            let (properties, flattened) =
                struct_field_properties(&variant.fields, rename_all_fields, false)?;
            let deny_unknown = serde_deny_unknown_fields(&variant.attrs);
            let none = quote! { ::core::option::Option::None };
            Ok(object_body(&properties, &flattened, deny_unknown, &none))
        }
    }
}

/// The subschema for a variant under serde's default (externally-tagged)
/// representation: a bare const for unit variants, otherwise a single-key object
/// `{ "<variant>": <payload> }`.
fn externally_tagged_variant_schema(
    variant: &syn::Variant,
    wire_name: &str,
    rename_all_fields: Option<&str>,
) -> syn::Result<proc_macro2::TokenStream> {
    match &variant.fields {
        Fields::Unit => Ok(quote! { ::runx_contracts::schema::const_string(#wire_name) }),
        Fields::Unnamed(fields) => {
            let inner = newtype_inner_schema(variant, fields)?;
            Ok(quote! {
                ::runx_contracts::schema::externally_tagged_variant(#wire_name, #inner)
            })
        }
        Fields::Named(_) => {
            let (properties, flattened) =
                struct_field_properties(&variant.fields, rename_all_fields, false)?;
            let deny_unknown = serde_deny_unknown_fields(&variant.attrs);
            let none = quote! { ::core::option::Option::None };
            let inner = object_body(&properties, &flattened, deny_unknown, &none);
            Ok(quote! {
                ::runx_contracts::schema::externally_tagged_variant(#wire_name, #inner)
            })
        }
    }
}

/// The subschema for a variant under `#[serde(tag = "...")]`: an object whose
/// tag field is `const`-pinned to the variant name, plus the struct variant's
/// fields. serde permits only struct and unit variants under internal tagging.
fn internally_tagged_variant_schema(
    variant: &syn::Variant,
    wire_name: &str,
    tag: &str,
    rename_all_fields: Option<&str>,
    container_deny: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut properties = Vec::new();
    let mut flattened: Vec<proc_macro2::TokenStream> = Vec::new();
    properties.push(quote! {
        ::runx_contracts::schema::Property::new(
            #tag,
            ::runx_contracts::schema::const_string(#wire_name),
            true,
        )
    });
    match &variant.fields {
        Fields::Unit => {}
        Fields::Named(_) => {
            let (field_props, field_flattened) =
                struct_field_properties(&variant.fields, rename_all_fields, false)?;
            properties.extend(field_props);
            flattened.extend(field_flattened);
        }
        Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "internally-tagged enums cannot have tuple variants (serde rejects this too)",
            ));
        }
    }
    let deny_unknown = container_deny || serde_deny_unknown_fields(&variant.attrs);
    if !flattened.is_empty() {
        let none = quote! { ::core::option::Option::None };
        return Ok(object_body(&properties, &flattened, deny_unknown, &none));
    }
    Ok(quote! {
        ::runx_contracts::schema::object_schema(
            ::std::vec![#(#properties),*],
            #deny_unknown,
            ::core::option::Option::None,
        )
    })
}

/// The payload schema for a single-field (newtype) tuple variant: the wrapped
/// type's own schema. Multi-field tuple variants are not modeled.
fn newtype_inner_schema(
    variant: &syn::Variant,
    fields: &syn::FieldsUnnamed,
) -> syn::Result<proc_macro2::TokenStream> {
    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            &variant.ident,
            "RunxSchema supports only single-field tuple variants (newtype variants)",
        ));
    }
    let inner_ty = &fields.unnamed[0].ty;
    Ok(quote! {
        <#inner_ty as ::runx_contracts::schema::RunxSchema>::json_schema()
    })
}

/// Strip a leading `Option<...>`, returning the inner type and whether it was
/// optional.
fn unwrap_option(ty: &Type) -> (Type, bool) {
    let Type::Path(type_path) = ty else {
        return (ty.clone(), false);
    };
    let Some(segment) = type_path.path.segments.last() else {
        return (ty.clone(), false);
    };
    if segment.ident != "Option" {
        return (ty.clone(), false);
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return (ty.clone(), false);
    };
    let Some(GenericArgument::Type(inner)) = arguments.args.first() else {
        return (ty.clone(), false);
    };
    (inner.clone(), true)
}

/// The top-level identity an emitted document carries, parsed from the
/// `#[runx_schema(...)]` attribute. Mirrors `runx_contracts::schema::Identity`.
enum Identity {
    /// `id = "runx.reference.v1"`, optionally with `url = "<full $id>"` when the
    /// canonical `$id` does not match the mechanical transform of the logical
    /// name.
    Runx {
        logical: String,
        url: Option<String>,
    },
    /// `spec_id = "https://runx.ai/spec/question.schema.json"`: a bare `$id`
    /// with no `x-runx-schema` marker (the legacy `runx.ai/*` documents).
    BareId { url: String },
}

/// `#[runx_schema(id = "runx.reference.v1")]`, `#[runx_schema(id = "...", url =
/// "...")]`, or `#[runx_schema(spec_id = "https://...")]` on the type, if
/// present.
fn runx_identity(attrs: &[syn::Attribute]) -> syn::Result<Option<Identity>> {
    for attr in attrs {
        if !attr.path().is_ident("runx_schema") {
            continue;
        }
        let mut logical: Option<String> = None;
        let mut url: Option<String> = None;
        let mut spec_id: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("id") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                logical = Some(lit.value());
                Ok(())
            } else if meta.path.is_ident("url") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                url = Some(lit.value());
                Ok(())
            } else if meta.path.is_ident("spec_id") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                spec_id = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("unsupported runx_schema attribute"))
            }
        })?;
        if let Some(url) = spec_id {
            return Ok(Some(Identity::BareId { url }));
        }
        if let Some(logical) = logical {
            return Ok(Some(Identity::Runx { logical, url }));
        }
    }
    Ok(None)
}

fn serde_rename_all(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    serde_string_value(attrs, "rename_all")
}

/// `#[serde(rename_all_fields = "...")]` on an enum: the case convention applied
/// to the fields of every struct variant.
fn serde_rename_all_fields(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    serde_string_value(attrs, "rename_all_fields")
}

fn serde_tag(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    serde_string_value(attrs, "tag")
}

fn serde_rename(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    serde_string_value(attrs, "rename")
}

/// Read a `#[serde(<key> = "value")]` string value if present.
fn serde_string_value(attrs: &[syn::Attribute], key: &str) -> syn::Result<Option<String>> {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let mut found = None;
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(key) {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                found = Some(lit.value());
                Ok(())
            } else {
                // Consume any value for keys we do not care about so parsing
                // does not error on `rename = "x"`, `default`, etc.
                if let Ok(value) = meta.value() {
                    let _: Lit = value.parse()?;
                }
                Ok(())
            }
        });
        // Ignore serde forms we do not model (e.g. bare path flags); only the
        // requested string value matters here.
        if parsed.is_ok() && found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

fn serde_flag(attrs: &[syn::Attribute], flag: &str) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let mut present = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(flag) {
                present = true;
            } else if let Ok(value) = meta.value() {
                let _: Lit = value.parse()?;
            }
            Ok(())
        });
        if present {
            return true;
        }
    }
    false
}

fn serde_deny_unknown_fields(attrs: &[syn::Attribute]) -> bool {
    serde_flag(attrs, "deny_unknown_fields")
}

/// A `#[serde(default)]` flag (the value form `default = "path"` also makes the
/// field optional, so accept either).
fn serde_default(attrs: &[syn::Attribute]) -> bool {
    serde_flag(attrs, "default")
}

fn serde_untagged(attrs: &[syn::Attribute]) -> bool {
    serde_flag(attrs, "untagged")
}

/// Whether a field carries `#[serde(skip_serializing_if = "...")]`. Such a field
/// can be omitted from the wire, so an `Option<T>` with it is optional rather
/// than required-but-nullable.
fn serde_skip_serializing_if(attrs: &[syn::Attribute]) -> bool {
    serde_string_value(attrs, "skip_serializing_if")
        .map(|value| value.is_some())
        .unwrap_or(false)
}

fn serde_skip(attrs: &[syn::Attribute]) -> bool {
    serde_flag(attrs, "skip") || serde_flag(attrs, "skip_serializing")
}

/// Apply a serde `rename_all` rule to an identifier. Covers the rules the
/// contract types use.
fn apply_rename_all(ident: &str, rule: Option<&str>) -> String {
    match rule {
        Some("lowercase") => ident.to_lowercase(),
        Some("UPPERCASE") => ident.to_uppercase(),
        Some("snake_case") => to_snake_case(ident),
        Some("camelCase") => to_camel_case(ident),
        Some("PascalCase") => to_pascal_case(ident),
        Some("kebab-case") => to_snake_case(ident).replace('_', "-"),
        Some("SCREAMING_SNAKE_CASE") => to_snake_case(ident).to_uppercase(),
        _ => ident.to_owned(),
    }
}

fn to_snake_case(ident: &str) -> String {
    let mut out = String::new();
    for (index, ch) in ident.chars().enumerate() {
        if ch.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn to_pascal_case(ident: &str) -> String {
    let mut out = String::new();
    let mut upper_next = true;
    for ch in ident.chars() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn to_camel_case(ident: &str) -> String {
    let pascal = to_pascal_case(ident);
    let mut chars = pascal.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => pascal,
    }
}
