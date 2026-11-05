use boa_engine::object::{FunctionObjectBuilder, IntegrityLevel, JsObject, ObjectInitializer};
use boa_engine::property::{Attribute, PropertyDescriptor};
use boa_engine::{Context, JsArgs, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use runx_contracts::javascript_worker::WorkerApi;
use url::Url;

pub(super) fn install(
    context: &mut Context,
    input_bytes: usize,
    output_bytes: usize,
) -> JsResult<()> {
    disable_randomness(context)?;
    install_runx_api(context, input_bytes, output_bytes)
}

fn disable_randomness(context: &mut Context) -> JsResult<()> {
    let unavailable = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(random_unavailable),
    )
    .name(js_string!("random"))
    .build();
    context
        .intrinsics()
        .objects()
        .math()
        .define_property_or_throw(
            js_string!("random"),
            PropertyDescriptor::builder()
                .value(unavailable)
                .writable(false)
                .enumerable(false)
                .configurable(false),
            context,
        )?;
    Ok(())
}

fn install_runx_api(
    context: &mut Context,
    input_bytes: usize,
    output_bytes: usize,
) -> JsResult<()> {
    let parse_url =
        FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(parse_url))
            .name(js_string!(WorkerApi::ParseUrl.name()))
            .length(1)
            .build();
    freeze(&parse_url, context)?;
    let parse_data = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_copy_closure_with_captures(parse_data, (input_bytes, output_bytes)),
    )
    .name(js_string!(WorkerApi::ParseData.name()))
    .length(2)
    .build();
    freeze(&parse_data, context)?;

    let runx = ObjectInitializer::new(context)
        .property(
            js_string!(WorkerApi::ParseUrl.name()),
            parse_url,
            Attribute::default(),
        )
        .property(
            js_string!(WorkerApi::ParseData.name()),
            parse_data,
            Attribute::default(),
        )
        .build();
    freeze(&runx, context)?;
    context.register_global_property(js_string!("Runx"), runx, Attribute::default())
}

fn parse_data(
    _this: &JsValue,
    arguments: &[JsValue],
    limits: &(usize, usize),
    context: &mut Context,
) -> JsResult<JsValue> {
    let source = arguments
        .get_or_undefined(0)
        .as_string()
        .ok_or_else(|| {
            JsNativeError::typ().with_message("Runx.parseData requires a source string")
        })?
        .to_std_string()
        .map_err(|_| JsNativeError::typ().with_message("Runx.parseData requires valid Unicode"))?;
    let format = arguments
        .get_or_undefined(1)
        .as_string()
        .ok_or_else(|| {
            JsNativeError::typ().with_message("Runx.parseData requires json, yaml or toml")
        })?
        .to_std_string()
        .map_err(|_| {
            JsNativeError::typ().with_message("Runx.parseData requires a valid format string")
        })?;
    // Source may be constructed by the module rather than supplied as input.
    if source.len() > limits.0 {
        return Err(JsNativeError::range()
            .with_message("Runx.parseData source exceeds its byte budget")
            .into());
    }
    let value = runx_parser::data::decode_data(&source, &format, limits.1)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    super::validation::validate_input(&value, limits.1)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    JsValue::from_json(&value, context)
}

fn random_unavailable(
    _this: &JsValue,
    _arguments: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Err(JsNativeError::typ()
        .with_message("Math.random is unavailable in deterministic modules")
        .into())
}

fn freeze(object: &JsObject, context: &mut Context) -> JsResult<()> {
    if object.set_integrity_level(IntegrityLevel::Frozen, context)? {
        return Ok(());
    }
    Err(JsNativeError::error()
        .with_message("failed to freeze the Runx deterministic API")
        .into())
}

fn parse_url(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let input = arguments
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let url = Url::parse(&input).map_err(|error| {
        JsNativeError::typ()
            .with_message(format!("Runx.parseUrl requires an absolute URL: {error}"))
    })?;
    let hostname = url
        .host()
        .map_or_else(String::new, |hostname| hostname.to_string());
    let value = serde_json::json!({
        "href": url.as_str(),
        "origin": url.origin().ascii_serialization(),
        "protocol": format!("{}:", url.scheme()),
        "hostname": hostname,
        "username": url.username(),
        "password": url.password().unwrap_or_default(),
    });
    JsValue::from_json(&value, context)
}
