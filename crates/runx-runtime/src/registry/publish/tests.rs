use std::cell::RefCell;

use super::*;
use crate::http::{
    RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};

#[test]
fn publish_skill_sends_the_complete_package_contract() -> Result<(), Box<dyn std::error::Error>> {
    let transport = StubTransport::new(successful_skill_response("success", "published"));
    let files = vec![RegistryPackageFile {
        path: "run.mjs".to_owned(),
        content: "console.log('hello');\n".to_owned(),
    }];
    let result = publish_hosted_skill(
        &transport,
        "https://runx.test/",
        "rxk_secret",
        &HostedSkillPublishRequest {
            markdown: "---\nname: hello\ndescription: hello fixture.\n---\nHello.\n",
            profile_document: Some("skill: hello\nrunners: {}\n"),
            version: Some("sha-123"),
            package_files: &files,
        },
    )?;

    assert_eq!(result.skill_id, "kam/hello");
    let requests = transport.requests.borrow();
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(requests[0].url, "https://runx.test/v1/skills");
    assert!(
        requests[0].headers.iter().any(|header| {
            header.name == "authorization" && header.value == "Bearer rxk_secret"
        })
    );
    let body = request_body(&requests[0])?;
    assert_eq!(body["version"], "sha-123");
    assert_eq!(body["package_files"][0]["path"], "run.mjs");
    Ok(())
}

#[test]
fn publish_skill_rejects_an_unsuccessful_success_status_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let transport = StubTransport::new(successful_skill_response("failure", "rejected"));
    let result = publish_hosted_skill(
        &transport,
        "https://runx.test",
        "rxk_secret",
        &HostedSkillPublishRequest {
            markdown: "---\nname: hello\ndescription: hello fixture.\n---\nHello.\n",
            profile_document: None,
            version: None,
            package_files: &[],
        },
    );

    assert!(matches!(result, Err(RegistryPublishError::Contract(_))));
    Ok(())
}

#[test]
fn publish_admin_sends_owner_and_harness_and_no_upsert() -> Result<(), Box<dyn std::error::Error>> {
    let transport = StubTransport::new(RuntimeHttpResponse::new(
        200,
        serde_json::json!({
            "status": "success",
            "publish": {
                "status": "published",
                "skill_id": "runx/hello",
                "name": "hello",
                "version": "sha-123",
                "digest": "abc",
                "profile_digest": "profile-abc",
                "link": {
                    "install_command": {"shell": "posix", "argv": ["runx", "add", "runx/hello@sha-123"], "copy_text": "runx add runx/hello@sha-123"},
                    "run_command": {"shell": "posix", "argv": ["runx", "skill", "runx/hello@sha-123"], "copy_text": "runx skill runx/hello@sha-123"}
                },
                "record": { "owner": "runx", "trust_tier": "first_party" }
            }
        })
        .to_string(),
    ));
    let files = vec![RegistryPackageFile {
        path: "run.mjs".to_owned(),
        content: "console.log('hello');\n".to_owned(),
    }];
    let harness = RegistryPublishHarnessReport {
        status: "passed".to_owned(),
        case_count: 1,
        assertion_error_count: 0,
        assertion_errors: Vec::new(),
        case_names: vec!["smoke".to_owned()],
        receipt_ids: vec!["rx_harness_1".to_owned()],
        graph_case_count: 0,
    };
    let result = publish_hosted_admin_skill(
        &transport,
        "https://runx.test/",
        "admin-token",
        &HostedAdminSkillPublishRequest {
            owner: "runx",
            markdown: "---\nname: hello\ndescription: hello fixture.\n---\nHello.\n",
            profile_document: Some("skill: hello\nrunners: {}\n"),
            version: Some("sha-123"),
            package_files: &files,
            harness: &harness,
        },
    )?;

    assert_eq!(result.owner, "runx");
    assert_eq!(result.public_url, "https://runx.ai/x/runx/hello@sha-123");
    let requests = transport.requests.borrow();
    assert_eq!(
        requests[0].url,
        "https://runx.test/v1/admin/registry/publish"
    );
    let body = request_body(&requests[0])?;
    assert_eq!(body["owner"], "runx");
    assert!(body.get("upsert").is_none());
    assert_eq!(body["harness"]["status"], "passed");
    Ok(())
}

fn successful_skill_response(status: &str, publish_status: &str) -> RuntimeHttpResponse {
    RuntimeHttpResponse::new(
        200,
        serde_json::json!({
            "status": status,
            "publish": {
                "status": publish_status,
                "skill_id": "kam/hello",
                "owner": "kam",
                "name": "hello",
                "version": "sha-123",
                "digest": "abc",
                "trust_tier": "community",
                "install_command": {"shell": "posix", "argv": ["runx", "add", "kam/hello@sha-123"], "copy_text": "runx add kam/hello@sha-123"},
                "run_command": {"shell": "posix", "argv": ["runx", "skill", "kam/hello@sha-123"], "copy_text": "runx skill kam/hello@sha-123"},
                "public_url": "https://runx.test/x/kam/hello"
            }
        })
        .to_string(),
    )
}

fn request_body(request: &RuntimeHttpRequest) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(request.body.as_deref().unwrap_or_default())
}

struct StubTransport {
    requests: RefCell<Vec<RuntimeHttpRequest>>,
    response: RefCell<Option<RuntimeHttpResponse>>,
}

impl StubTransport {
    fn new(response: RuntimeHttpResponse) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            response: RefCell::new(Some(response)),
        }
    }
}

impl RuntimeHttpTransport for StubTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.requests.borrow_mut().push(request);
        self.response
            .borrow_mut()
            .take()
            .ok_or_else(|| RuntimeHttpError::Transport {
                message: "missing stub response".to_owned(),
            })
    }
}
