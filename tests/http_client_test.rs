// =============================================================================
// Integration Tests — HttpClient with auth injection
// =============================================================================

use mockito::{Matcher, Server};
use restie::config::{AuthConfig, SiteConfig, SiteInfo};
use restie::http_client::HttpClient;
use serde_json::json;
use std::collections::HashMap;

fn make_client(server: &Server) -> HttpClient {
    HttpClient::with_base(&server.url())
}

fn make_client_with_auth(server: &Server, auth: AuthConfig) -> HttpClient {
    let config = SiteConfig {
        site: Some(SiteInfo {
            name: Some("test".to_string()),
            base_url: Some(server.url()),
            auth: Some(auth),
            headers: None,
        }),
        modules: std::collections::BTreeMap::new(),
    };
    HttpClient::new(&config)
}

#[tokio::test]
async fn test_get_returns_json() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/test")
        .with_status(200)
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client.GET("/test", None).await;

    mock.assert_async().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["ok"], true);
}

#[tokio::test]
async fn test_get_with_query() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/search")
        .match_query(Matcher::UrlEncoded("q".into(), "rust".into()))
        .with_status(200)
        .with_body(r#"{"count":1}"#)
        .create_async()
        .await;

    let client = make_client(&server);
    let mut query = HashMap::new();
    query.insert("q".to_string(), "rust".to_string());
    let result = client.GET("/search", Some(query)).await;

    mock.assert_async().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["count"], 1);
}

#[tokio::test]
async fn test_post_with_body() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/create")
        .match_body(Matcher::PartialJson(json!({"name": "test"})))
        .with_status(201)
        .with_body(r#"{"id":1}"#)
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client
        .POST("/create", Some(json!({"name": "test"})), None)
        .await;

    mock.assert_async().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["id"], 1);
}

#[tokio::test]
async fn test_put_method() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("PUT", "/update/1")
        .match_body(Matcher::PartialJson(json!({"active": true})))
        .with_status(200)
        .with_body(r#"{"updated":true}"#)
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client
        .PUT("/update/1", Some(json!({"active": true})), None)
        .await;

    mock.assert_async().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["updated"], true);
}

#[tokio::test]
async fn test_patch_method() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("PATCH", "/patch/1")
        .match_body(Matcher::PartialJson(json!({"field": "val"})))
        .with_status(200)
        .with_body(r#"{"patched":true}"#)
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client
        .PATCH("/patch/1", Some(json!({"field": "val"})), None)
        .await;

    mock.assert_async().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["patched"], true);
}

#[tokio::test]
async fn test_delete_method() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("DELETE", "/delete/1")
        .with_status(204)
        .with_body("")
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client.DELETE("/delete/1", None).await;

    mock.assert_async().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_bearer_token_auth_header() {
    std::env::set_var("TEST_TOKEN_D", "ghp-secret-token");
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/auth")
        .match_header("authorization", "Bearer ghp-secret-token")
        .with_status(200)
        .with_body("{}")
        .create_async()
        .await;

    let auth = AuthConfig {
        auth_type: "bearer_token".to_string(),
        token_env: Some("TEST_TOKEN_D".to_string()),
        header: None,
        prefix: None,
        command: None,
        env: None,
    };
    let client = make_client_with_auth(&server, auth);
    let _ = client.GET("/auth", None).await;

    mock.assert_async().await;
}

#[tokio::test]
async fn test_custom_header_auth() {
    std::env::set_var("TEST_TOKEN_E", "glpat-token");
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/auth")
        .match_header("private-token", "glpat-token")
        .with_status(200)
        .with_body("{}")
        .create_async()
        .await;

    let auth = AuthConfig {
        auth_type: "custom_header".to_string(),
        token_env: Some("TEST_TOKEN_E".to_string()),
        header: Some("PRIVATE-TOKEN".to_string()),
        prefix: None,
        command: None,
        env: None,
    };
    let client = make_client_with_auth(&server, auth);
    let _ = client.GET("/auth", None).await;

    mock.assert_async().await;
}

#[tokio::test]
async fn test_sends_user_agent_header() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/ua")
        .match_header("user-agent", "restie/0.1.0")
        .with_status(200)
        .with_body("{}")
        .create_async()
        .await;

    let client = make_client(&server);
    let _ = client.GET("/ua", None).await;

    mock.assert_async().await;
}

#[tokio::test]
async fn test_error_404() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/not-found")
        .with_status(404)
        .with_body(r#"{"message":"Not Found"}"#)
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client.GET("/not-found", None).await;

    mock.assert_async().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Not Found"));
}

#[tokio::test]
async fn test_non_json_response() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/text")
        .with_status(200)
        .with_body("plain text")
        .create_async()
        .await;

    let client = make_client(&server);
    let result = client.GET("/text", None).await;

    mock.assert_async().await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        serde_json::Value::String("plain text".to_string())
    );
}
