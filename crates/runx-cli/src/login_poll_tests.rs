use super::*;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use runx_runtime::{RuntimeHttpRequest, RuntimeHttpResponse};

const BUDGET: Duration = Duration::from_secs(DEFAULT_LOGIN_TIMEOUT_SECONDS);

struct PollFixture {
    now: Cell<Instant>,
    requests: RefCell<Vec<RuntimeHttpRequest>>,
    responses: RefCell<VecDeque<RuntimeHttpResponse>>,
    request_elapsed: Cell<Duration>,
    sleeps: RefCell<Vec<Duration>>,
    sleep_overrun: Cell<Duration>,
}

impl PollFixture {
    fn new(responses: Vec<RuntimeHttpResponse>) -> Self {
        Self {
            now: Cell::new(Instant::now()),
            requests: RefCell::new(Vec::new()),
            responses: RefCell::new(responses.into()),
            request_elapsed: Cell::new(Duration::ZERO),
            sleeps: RefCell::new(Vec::new()),
            sleep_overrun: Cell::new(Duration::ZERO),
        }
    }

    fn run(
        &self,
        poll_after_ms: Option<u64>,
        deadline: Instant,
    ) -> Result<HostedLoginCompleteResponse, LoginCliError> {
        wait_for_login_completion_until(
            self,
            "https://runx.test",
            &HostedLoginStartResponse {
                status: "pending".to_owned(),
                session_id: "login_fixture".to_owned(),
                login_token: "fixture_poll_ticket".to_owned(),
                authorization_url: Some("https://runx.test/connect/login_fixture".to_owned()),
                poll_after_ms,
            },
            &|duration| {
                self.sleeps.borrow_mut().push(duration);
                self.now.set(self.now.get() + duration + self.sleep_overrun.get());
            },
            &|| self.now.get(),
            deadline,
        )
    }
}

impl Transport for PollFixture {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.requests.borrow_mut().push(request);
        self.now.set(self.now.get() + self.request_elapsed.get());
        Ok(self.responses.borrow_mut().pop_front().unwrap_or_else(|| {
            RuntimeHttpResponse::new(500, "missing login polling fixture response")
        }))
    }
}

fn pending(poll_after_ms: Option<u64>) -> RuntimeHttpResponse {
    RuntimeHttpResponse::new(202, serde_json::json!({
        "status": "pending",
        "session_id": "login_fixture",
        "poll_after_ms": poll_after_ms
    }).to_string())
}

fn success() -> RuntimeHttpResponse {
    RuntimeHttpResponse::new(200, serde_json::json!({
        "status": "success",
        "session_id": "login_fixture",
        "principal_id": "user_fixture",
        "credential_id": "cred_fixture",
        "token": "rxk_fixture_login"
    }).to_string())
}

#[test]
fn expired_deadline_does_not_start_a_request() {
    let fixture = PollFixture::new(vec![success()]);
    let result = fixture.run(None, fixture.now.get());
    assert!(matches!(result, Err(LoginCliError::LoginTimedOut)));
    assert!(fixture.requests.borrow().is_empty());
    assert!(fixture.sleeps.borrow().is_empty());
}

#[test]
fn large_initial_interval_is_clamped_without_a_late_poll() {
    for interval in [600_000, u64::MAX] {
        let fixture = PollFixture::new(vec![pending(None)]);
        let deadline = fixture.now.get() + BUDGET;
        let result = fixture.run(Some(interval), deadline);
        assert!(matches!(result, Err(LoginCliError::LoginTimedOut)));
        assert_eq!(fixture.requests.borrow().len(), 1);
        assert_eq!(*fixture.sleeps.borrow(), vec![BUDGET]);
        assert_eq!(fixture.now.get(), deadline);
    }
}

#[test]
fn pending_response_cannot_extend_the_remaining_sleep_budget() {
    let fixture = PollFixture::new(vec![pending(Some(600_000))]);
    fixture.request_elapsed.set(Duration::from_secs(30));
    let deadline = fixture.now.get() + BUDGET;
    let result = fixture.run(Some(1_000), deadline);
    assert!(matches!(result, Err(LoginCliError::LoginTimedOut)));
    assert_eq!(fixture.requests.borrow().len(), 1);
    assert_eq!(*fixture.sleeps.borrow(), vec![Duration::from_secs(150)]);
    assert_eq!(fixture.now.get(), deadline);
}

#[test]
fn changing_intervals_share_one_deadline() {
    let fixture = PollFixture::new(vec![
        pending(Some(50_000)),
        pending(Some(60_000)),
        pending(Some(80_000)),
    ]);
    let deadline = fixture.now.get() + BUDGET;
    let result = fixture.run(None, deadline);
    assert!(matches!(result, Err(LoginCliError::LoginTimedOut)));
    assert_eq!(fixture.requests.borrow().len(), 3);
    assert_eq!(*fixture.sleeps.borrow(), vec![
        Duration::from_secs(50),
        Duration::from_secs(60),
        Duration::from_secs(70),
    ]);
    assert_eq!(fixture.now.get(), deadline);
}

#[test]
fn scheduler_oversleep_does_not_start_another_request() {
    let fixture = PollFixture::new(vec![pending(Some(600_000))]);
    fixture.sleep_overrun.set(Duration::from_secs(1));
    let result = fixture.run(None, fixture.now.get() + BUDGET);
    assert!(matches!(result, Err(LoginCliError::LoginTimedOut)));
    assert_eq!(fixture.requests.borrow().len(), 1);
    assert_eq!(*fixture.sleeps.borrow(), vec![BUDGET]);
}

#[test]
fn pending_request_consuming_the_budget_does_not_sleep() {
    let fixture = PollFixture::new(vec![pending(Some(1_000))]);
    fixture.request_elapsed.set(BUDGET);
    let result = fixture.run(None, fixture.now.get() + BUDGET);
    assert!(matches!(result, Err(LoginCliError::LoginTimedOut)));
    assert_eq!(fixture.requests.borrow().len(), 1);
    assert!(fixture.sleeps.borrow().is_empty());
}

#[test]
fn successful_completion_preserves_default_and_zero_intervals()
-> Result<(), Box<dyn std::error::Error>> {
    for (interval, expected_sleep) in [
        (None, Duration::from_secs(1)),
        (Some(0), Duration::ZERO),
    ] {
        let fixture = PollFixture::new(vec![pending(None), success()]);
        let result = fixture.run(interval, fixture.now.get() + BUDGET)?;
        assert_eq!(result.status, "success");
        assert_eq!(fixture.requests.borrow().len(), 2);
        assert_eq!(*fixture.sleeps.borrow(), vec![expected_sleep]);
    }
    Ok(())
}

#[test]
fn in_flight_success_keeps_existing_semantics() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = PollFixture::new(vec![success()]);
    fixture.request_elapsed.set(BUDGET + Duration::from_secs(1));
    let deadline = fixture.now.get() + BUDGET;
    let result = fixture.run(None, deadline)?;
    assert_eq!(result.status, "success");
    assert!(fixture.now.get() > deadline);
    assert_eq!(fixture.requests.borrow().len(), 1);
    assert!(fixture.sleeps.borrow().is_empty());
    Ok(())
}

#[test]
fn api_error_is_not_replaced_by_a_polling_retry() {
    let fixture = PollFixture::new(vec![RuntimeHttpResponse::new(400, serde_json::json!({
        "status": "error",
        "error": { "code": "login_expired", "detail": "fixture session expired" }
    }).to_string())]);
    let result = fixture.run(None, fixture.now.get() + BUDGET);
    assert!(matches!(result, Err(LoginCliError::Http(_))));
    assert_eq!(fixture.requests.borrow().len(), 1);
    assert!(fixture.sleeps.borrow().is_empty());
}
