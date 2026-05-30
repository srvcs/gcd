use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-gcd";
pub const CONCERN: &str = "number theory: greatest common divisor";
pub const DEPENDS_ON: &[&str] = &["srvcs-modulo", "srvcs-iszero"];

/// Upper bound on Euclidean iterations. The algorithm terminates in
/// O(log min(a,b)) steps for well-behaved dependencies; this cap defends against
/// a misbehaving `srvcs-modulo`/`srvcs-iszero` that never converges, surfacing a
/// `500` instead of looping forever.
const MAX_ITERATIONS: usize = 1000;

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub modulo_url: String,
    pub iszero_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    pub a: i64,
    pub b: i64,
}

#[derive(Serialize, ToSchema)]
pub struct GcdResponse {
    pub a: i64,
    pub b: i64,
    pub result: i64,
}

fn ok(a: i64, b: i64, result: i64) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "a": a, "b": b, "result": result })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

fn loop_exhausted() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "euclidean loop did not converge" })),
    )
        .into_response()
}

/// Call one dependency at `url` with `body`, mapping its outcome to either the
/// parsed response body (on `200`) or an early-return `Response` the caller
/// should surface verbatim:
///
/// - unreachable / non-`200`/`422` -> `503` degraded
/// - `422` -> forwarded `422` (the dependency rejected the input)
async fn ask(url: &str, body: &Value, dependency: &str) -> Result<Value, Response> {
    match client::call(url, body).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => Ok(body),
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — compute `gcd(a, b)` via the Euclidean algorithm.
///
/// This service owns the *control flow* (an iterative Euclidean loop) but
/// delegates every primitive to its dependencies: it asks `srvcs-iszero`
/// whether the divisor has reached zero and `srvcs-modulo` for each
/// `a mod b`. `gcd(a, 0) == a` and `gcd(0, 0) == 0` fall out naturally, since
/// the first `iszero` check breaks immediately when `b == 0`.
///
/// If a dependency is unreachable it reports itself degraded (`503`); if a
/// dependency rejects the input it forwards the `422`; and if the loop fails to
/// converge within the iteration cap it returns `500` rather than spinning.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = GcdResponse),
        (status = 422, description = "a dependency rejected the input (forwarded)"),
        (status = 500, description = "the euclidean loop did not converge"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    let (a, b) = (req.a, req.b);
    let mut x = a;
    let mut y = b;

    for _ in 0..MAX_ITERATIONS {
        // Is the current divisor zero? If so, x is the gcd.
        let iszero_body = match ask(&deps.iszero_url, &json!({ "value": y }), "srvcs-iszero").await
        {
            Ok(body) => body,
            Err(resp) => return resp,
        };
        let is_zero = iszero_body
            .get("result")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_zero {
            return ok(a, b, x);
        }

        // r = x mod y, delegated to srvcs-modulo.
        let modulo_body =
            match ask(&deps.modulo_url, &json!({ "a": x, "b": y }), "srvcs-modulo").await {
                Ok(body) => body,
                Err(resp) => return resp,
            };
        let r = match modulo_body.get("result").and_then(Value::as_i64) {
            Some(r) => r,
            None => return degraded("srvcs-modulo"),
        };

        x = y;
        y = r;
    }

    loop_exhausted()
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, GcdResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[tokio::test]
    async fn index_reports_both_dependencies() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-gcd");
        assert_eq!(info.concern, "number theory: greatest common divisor");
        assert_eq!(info.depends_on, vec!["srvcs-modulo", "srvcs-iszero"]);
    }
}
