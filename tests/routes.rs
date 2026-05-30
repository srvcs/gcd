use axum::body::Body;
use axum::extract::Json as AxumJson;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_gcd::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

const DEAD_URL: &str = "http://127.0.0.1:1";

/// Spawn a *computing* mock `srvcs-iszero`: it reads `{"value": n}` and returns
/// `{"result": n == 0}` — the real answer, so the Euclidean loop's termination
/// is genuinely driven by the dependency rather than a canned response.
async fn spawn_iszero() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|AxumJson(body): AxumJson<Value>| async move {
            let n = body.get("value").and_then(Value::as_i64).unwrap_or(0);
            Json(json!({ "result": n == 0 }))
        }),
    );
    serve(app).await
}

/// Spawn a *computing* mock `srvcs-modulo`: it reads `{"a": x, "b": y}` and
/// returns `{"result": x % y}` — the real remainder.
async fn spawn_modulo() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|AxumJson(body): AxumJson<Value>| async move {
            let a = body.get("a").and_then(Value::as_i64).unwrap_or(0);
            let b = body.get("b").and_then(Value::as_i64).unwrap_or(1);
            if b == 0 {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({ "error": "modulo by zero" })),
                );
            }
            (StatusCode::OK, Json(json!({ "result": a % b })))
        }),
    );
    serve(app).await
}

/// Spawn a mock returning a fixed status + body (used for the 422-forward case).
async fn spawn_fixed(status: StatusCode, body: Value) -> String {
    let app = AxumRouter::new().route(
        "/",
        post(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    serve(app).await
}

async fn serve(app: AxumRouter) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(modulo_url: &str, iszero_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            modulo_url: modulo_url.to_string(),
            iszero_url: iszero_url.to_string(),
        },
    )
}

async fn gcd(modulo_url: &str, iszero_url: &str, a: i64, b: i64) -> (StatusCode, Value) {
    let res = app(modulo_url, iszero_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "a": a, "b": b }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL, DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn gcd_12_8_is_4() {
    let modulo = spawn_modulo().await;
    let iszero = spawn_iszero().await;
    let (status, body) = gcd(&modulo, &iszero, 12, 8).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["a"], 12);
    assert_eq!(body["b"], 8);
    assert_eq!(body["result"], 4);
}

#[tokio::test]
async fn gcd_17_5_is_1() {
    let modulo = spawn_modulo().await;
    let iszero = spawn_iszero().await;
    let (status, body) = gcd(&modulo, &iszero, 17, 5).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], 1);
}

#[tokio::test]
async fn gcd_10_0_is_10() {
    // gcd(a, 0) == a: the first iszero check breaks immediately, modulo is never called.
    let modulo = spawn_modulo().await;
    let iszero = spawn_iszero().await;
    let (status, body) = gcd(&modulo, &iszero, 10, 0).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], 10);
}

#[tokio::test]
async fn gcd_0_0_is_0() {
    let modulo = spawn_modulo().await;
    let iszero = spawn_iszero().await;
    let (status, body) = gcd(&modulo, &iszero, 0, 0).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], 0);
}

#[tokio::test]
async fn forwards_422_from_modulo() {
    // iszero says y != 0 (computing), but modulo rejects the input -> forward 422.
    let iszero = spawn_iszero().await;
    let modulo = spawn_fixed(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({ "error": "bad operand" }),
    )
    .await;
    let (status, _) = gcd(&modulo, &iszero, 12, 8).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn degrades_when_iszero_unreachable() {
    let modulo = spawn_modulo().await;
    let (status, body) = gcd(&modulo, DEAD_URL, 12, 8).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-iszero");
}

#[tokio::test]
async fn degrades_when_modulo_unreachable() {
    // iszero is reachable and says y != 0, so the loop reaches the modulo call.
    let iszero = spawn_iszero().await;
    let (status, body) = gcd(DEAD_URL, &iszero, 12, 8).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-modulo");
}
