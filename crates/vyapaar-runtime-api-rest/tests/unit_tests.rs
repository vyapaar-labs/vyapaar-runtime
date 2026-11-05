use vyapaar_runtime_api_rest::openapi_json;

#[test]
fn openapi_document_is_servable() {
    let doc = openapi_json();
    assert!(doc.get("openapi").is_some());
    assert!(doc.get("paths").is_some());
    assert!(doc.get("info").is_some());
}
