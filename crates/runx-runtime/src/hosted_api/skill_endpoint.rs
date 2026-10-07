use url::Url;

use crate::http::{
    HttpMethod, ReqwestHttpTransport, RuntimeHttpError, RuntimeHttpHeader, RuntimeHttpRequest,
    RuntimeHttpResponse, RuntimeHttpTransport,
};

#[derive(Clone, Debug)]
pub struct HostedSkillChallenge {
    pub resource_url: String,
    pub response: RuntimeHttpResponse,
}

#[derive(Debug, thiserror::Error)]
pub enum HostedSkillEndpointError {
    #[error(transparent)]
    Http(#[from] RuntimeHttpError),
    #[error("invalid hosted skill id: {0}")]
    InvalidSkillId(String),
    #[error("invalid hosted skill base URL: {0}")]
    InvalidBaseUrl(String),
}

/// Request the challenge for one hosted skill endpoint. The runtime owns URL
/// construction and guarded transport selection; callers own presentation of
/// the returned protocol payload.
pub fn request_hosted_skill_challenge(
    base_url: &str,
    skill_id: &str,
    runner: &str,
    allow_private_network: bool,
) -> Result<HostedSkillChallenge, HostedSkillEndpointError> {
    let transport = if allow_private_network {
        ReqwestHttpTransport::with_private_network_access()?
    } else {
        ReqwestHttpTransport::new()?
    };
    request_hosted_skill_challenge_with_transport(&transport, base_url, skill_id, runner)
}

fn request_hosted_skill_challenge_with_transport<T: RuntimeHttpTransport + ?Sized>(
    transport: &T,
    base_url: &str,
    skill_id: &str,
    runner: &str,
) -> Result<HostedSkillChallenge, HostedSkillEndpointError> {
    let resource_url = hosted_skill_resource_url(base_url, skill_id)?;
    let response = transport.send(RuntimeHttpRequest {
        method: HttpMethod::Post,
        url: resource_url.clone(),
        headers: vec![
            RuntimeHttpHeader::new("content-type", "application/json"),
            RuntimeHttpHeader::new("x-runx-skill-runner", runner),
        ],
        body: Some(serde_json::json!({ "runner": runner }).to_string()),
    })?;
    Ok(HostedSkillChallenge {
        resource_url,
        response,
    })
}

fn hosted_skill_resource_url(
    base_url: &str,
    skill_id: &str,
) -> Result<String, HostedSkillEndpointError> {
    let (owner, name) = crate::registry::split_skill_id(skill_id)
        .map_err(|error| HostedSkillEndpointError::InvalidSkillId(error.to_string()))?;
    let mut url = Url::parse(base_url)
        .map_err(|error| HostedSkillEndpointError::InvalidBaseUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(HostedSkillEndpointError::InvalidBaseUrl(
            "URL must use HTTP(S) and support path segments".to_owned(),
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|_| {
            HostedSkillEndpointError::InvalidBaseUrl(
                "URL cannot carry hosted skill routes".to_owned(),
            )
        })?
        .pop_if_empty()
        .extend(["v1", "skills", owner, name, "run"]);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    struct StubTransport {
        response: RuntimeHttpResponse,
        requests: RefCell<Vec<RuntimeHttpRequest>>,
    }

    impl RuntimeHttpTransport for StubTransport {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            self.requests.borrow_mut().push(request);
            Ok(self.response.clone())
        }
    }

    #[test]
    fn hosted_skill_challenge_uses_the_runtime_transport() -> Result<(), Box<dyn std::error::Error>>
    {
        let transport = StubTransport {
            response: RuntimeHttpResponse::new(402, "{}"),
            requests: RefCell::new(Vec::new()),
        };

        let challenge = request_hosted_skill_challenge_with_transport(
            &transport,
            "https://api.runx.test/registry?ignored=yes#fragment",
            "ausca/document-ocr",
            "invoke",
        )?;

        assert_eq!(
            challenge.resource_url,
            "https://api.runx.test/registry/v1/skills/ausca/document-ocr/run"
        );
        let requests = transport.requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, HttpMethod::Post);
        assert_eq!(requests[0].url, challenge.resource_url);
        assert_eq!(
            requests[0].headers,
            vec![
                RuntimeHttpHeader::new("content-type", "application/json"),
                RuntimeHttpHeader::new("x-runx-skill-runner", "invoke"),
            ]
        );
        assert_eq!(requests[0].body.as_deref(), Some(r#"{"runner":"invoke"}"#));
        Ok(())
    }

    #[test]
    fn hosted_skill_challenge_rejects_invalid_identity_before_transport() {
        let transport = StubTransport {
            response: RuntimeHttpResponse::new(402, "{}"),
            requests: RefCell::new(Vec::new()),
        };

        let result = request_hosted_skill_challenge_with_transport(
            &transport,
            "https://api.runx.test",
            "../escape",
            "invoke",
        );

        assert!(matches!(
            result,
            Err(HostedSkillEndpointError::InvalidSkillId(_))
        ));
        assert!(transport.requests.borrow().is_empty());
    }
}
