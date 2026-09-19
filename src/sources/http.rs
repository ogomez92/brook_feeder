use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;

use crate::errors::{FeederError, FeederResult};

/// Attempts per request: one retry for failures that are usually a blip
/// (a dropped connection, a timeout, a 5xx or 429 from the server).
const ATTEMPTS: u32 = 2;

/// GET `url`, retrying transient failures, and fail on any non-2xx status.
///
/// Without the status check an error page gets handed to the feed parser,
/// which reports the HTML as a malformed feed ("no root element") instead of
/// saying the server answered 404.
pub fn get(client: &Client, url: &str) -> FeederResult<Response> {
    let mut attempt = 1;
    loop {
        let result = client.get(url).send();
        let transient = match &result {
            Ok(response) => {
                let status = response.status();
                status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS
            }
            Err(e) => e.is_timeout() || e.is_connect() || e.is_request(),
        };

        if transient && attempt < ATTEMPTS {
            std::thread::sleep(Duration::from_secs(2 * attempt as u64));
            attempt += 1;
            continue;
        }

        let response = result?;
        if !response.status().is_success() {
            return Err(FeederError::HttpStatus(response.status()));
        }
        return Ok(response);
    }
}
