use super::SkillPackageError;

mod lexer;

use lexer::{Token, tokenize};

pub(super) fn module_imports(path: &str, source: &str) -> Result<Vec<String>, SkillPackageError> {
    let tokens = tokenize(path, source)?;
    reject_effectful_module_tokens(path, &tokens)?;
    imports_from_tokens(path, &tokens)
}

/// Collect static module dependencies for a process-backed JavaScript tool.
/// Unlike deterministic modules, CLI tools may use process and Node APIs; the
/// package validator still needs the complete static import closure.
pub(super) fn process_module_imports(
    path: &str,
    source: &str,
) -> Result<Vec<String>, SkillPackageError> {
    let tokens = tokenize(path, source)?;
    let mut imports = imports_from_tokens(path, &tokens)?;
    collect_static_requires(path, &tokens, &mut imports)?;
    imports.sort();
    imports.dedup();
    Ok(imports)
}

fn collect_static_requires(
    path: &str,
    tokens: &[Token],
    imports: &mut Vec<String>,
) -> Result<(), SkillPackageError> {
    for (index, token) in tokens.iter().enumerate() {
        if !matches!(token, Token::Ident(identifier) if identifier == "require")
            || matches!(tokens.get(index.wrapping_sub(1)), Some(Token::Punct('.')))
            || !matches!(tokens.get(index + 1), Some(Token::Punct('(')))
        {
            continue;
        }
        match (tokens.get(index + 2), tokens.get(index + 3)) {
            (
                Some(Token::String {
                    value,
                    escaped: false,
                }),
                Some(Token::Punct(')')),
            ) => imports.push(value.clone()),
            (Some(Token::String { escaped: true, .. }), _) => {
                return Err(SkillPackageError::invalid(
                    path,
                    "CommonJS require specifiers must not use string escapes",
                ));
            }
            _ => {
                return Err(SkillPackageError::invalid(
                    path,
                    "process-backed JavaScript may use only static require(\"specifier\") dependencies",
                ));
            }
        }
    }
    Ok(())
}

fn imports_from_tokens(path: &str, tokens: &[Token]) -> Result<Vec<String>, SkillPackageError> {
    let mut imports = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        match tokens.get(index) {
            Some(Token::Ident(keyword)) if keyword == "import" => {
                if matches!(tokens.get(index.wrapping_sub(1)), Some(Token::Punct('.'))) {
                    index += 1;
                    continue;
                }
                index = parse_import(path, tokens, index, &mut imports)?;
            }
            Some(Token::Ident(keyword)) if keyword == "export" => {
                index = parse_export(path, tokens, index, &mut imports)?;
            }
            _ => index += 1,
        }
    }
    imports.sort();
    imports.dedup();
    Ok(imports)
}

fn reject_effectful_module_tokens(path: &str, tokens: &[Token]) -> Result<(), SkillPackageError> {
    for (index, token) in tokens.iter().enumerate() {
        let Token::Ident(identifier) = token else {
            continue;
        };
        let next = tokens.get(index + 1);
        if matches!(identifier.as_str(), "fetch" | "require")
            && matches!(next, Some(Token::Punct('(')))
        {
            return Err(effectful_module_error(path, identifier));
        }
        if matches!(identifier.as_str(), "RUNX_INPUTS_JSON" | "RUNX_INPUTS_PATH") {
            return Err(effectful_module_error(path, identifier));
        }
        if identifier == "process"
            && matches!(next, Some(Token::Punct('.')))
            && matches!(
                tokens.get(index + 2),
                Some(Token::Ident(field)) if matches!(field.as_str(), "env" | "stdout" | "stderr")
            )
        {
            return Err(effectful_module_error(path, "process runtime plumbing"));
        }
    }
    Ok(())
}

fn effectful_module_error(path: &str, boundary: &str) -> SkillPackageError {
    SkillPackageError::invalid(
        path,
        format!(
            "worker modules cannot own {boundary}; reach it through an operation step or declare a process tool"
        ),
    )
}

fn parse_import(
    path: &str,
    tokens: &[Token],
    index: usize,
    imports: &mut Vec<String>,
) -> Result<usize, SkillPackageError> {
    match tokens.get(index + 1) {
        Some(Token::Punct('(')) => Err(SkillPackageError::invalid(
            path,
            "dynamic import() is not available in deterministic JavaScript modules",
        )),
        Some(Token::String {
            value,
            escaped: false,
        }) => {
            imports.push(value.clone());
            Ok(index + 2)
        }
        Some(Token::String { escaped: true, .. }) => Err(SkillPackageError::invalid(
            path,
            "JavaScript module specifiers must not use string escapes",
        )),
        _ => parse_from_clause(path, tokens, index + 1, imports),
    }
}

fn parse_export(
    path: &str,
    tokens: &[Token],
    index: usize,
    imports: &mut Vec<String>,
) -> Result<usize, SkillPackageError> {
    match tokens.get(index + 1) {
        Some(Token::Punct('{' | '*')) => parse_from_clause(path, tokens, index + 1, imports),
        _ => Ok(index + 1),
    }
}

fn parse_from_clause(
    path: &str,
    tokens: &[Token],
    start: usize,
    imports: &mut Vec<String>,
) -> Result<usize, SkillPackageError> {
    let limit = start.saturating_add(64).min(tokens.len());
    let mut index = start;
    while index < limit {
        match tokens.get(index) {
            Some(Token::Punct(';')) => return Ok(index + 1),
            Some(Token::Ident(value)) if value == "from" => match tokens.get(index + 1) {
                Some(Token::String {
                    value,
                    escaped: false,
                }) => {
                    imports.push(value.clone());
                    return Ok(index + 2);
                }
                Some(Token::String { escaped: true, .. }) => {
                    return Err(SkillPackageError::invalid(
                        path,
                        "JavaScript module specifiers must not use string escapes",
                    ));
                }
                _ => {
                    return Err(SkillPackageError::invalid(
                        path,
                        "JavaScript import/export from must be followed by a plain string literal",
                    ));
                }
            },
            Some(Token::Ident(value)) if matches!(value.as_str(), "import" | "export") => {
                return Ok(index);
            }
            _ => index += 1,
        }
    }
    Ok(index.max(start + 1))
}

#[cfg(test)]
mod tests;
