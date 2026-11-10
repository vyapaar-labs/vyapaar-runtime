//! OpenAPI 3.1 schema generation for the Vyapaar REST API.
//!
//! Generates a comprehensive OpenAPI 3.1 document describing all REST endpoints,
//! request/response schemas, authentication schemes, and error responses.
//! Uses `schemars` for JSON Schema generation from Rust types.

use axum::response::IntoResponse;
use schemars::schema::*;
use schemars::schema_for;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use vyapaar_core::{InstrumentId, OrderSide, OrderStatus, OrderType, Price, Quantity, TimeInForce};
use vyapaar_risk::{RiskLimits, RiskRefusal};

/// OpenAPI 3.1 info block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiInfo {
    pub title: String,
    pub version: String,
    pub description: String,
}

/// OpenAPI 3.1 server object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiServer {
    pub url: String,
    pub description: String,
}

/// OpenAPI 3.1 security scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SecurityScheme {
    Http {
        scheme: String,
        bearer_format: Option<String>,
    },
    ApiKey {
        name: String,
        in_: String,
    },
}

/// OpenAPI 3.1 security requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityRequirement {
    #[serde(flatten)]
    pub scheme: BTreeMap<String, Vec<String>>,
}

/// Complete OpenAPI 3.1 document for the Vyapaar REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiDocument {
    pub openapi: String,
    pub info: OpenApiInfo,
    pub servers: Vec<OpenApiServer>,
    pub paths: BTreeMap<String, PathItem>,
    pub components: Components,
    pub tags: Vec<Tag>,
}

/// A single path item in the OpenAPI document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathItem {
    pub get: Option<Operation>,
    pub post: Option<Operation>,
    pub put: Option<Operation>,
    pub patch: Option<Operation>,
    pub delete: Option<Operation>,
}

/// An OpenAPI operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub tags: Vec<String>,
    pub summary: String,
    pub operation_id: String,
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    pub responses: BTreeMap<String, Response>,
    pub security: Vec<SecurityRequirement>,
}

/// An OpenAPI parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    pub name: String,
    pub r#in: String,
    pub required: bool,
    pub schema: Schema,
}

/// An OpenAPI request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    pub required: bool,
    pub content: BTreeMap<String, MediaType>,
}

/// An OpenAPI media type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaType {
    pub schema: Schema,
}

/// An OpenAPI response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub description: String,
    pub content: BTreeMap<String, MediaType>,
}

/// An OpenAPI tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub description: String,
}

/// Builds an OpenAPI operation with a JSON request/response body.
fn operation(
    tag: &str,
    summary: &str,
    operation_id: &str,
    parameters: Vec<Parameter>,
    request_body: Option<bool>,
    response_description: &str,
) -> Operation {
    Operation {
        tags: vec![tag.to_string()],
        summary: summary.to_string(),
        operation_id: operation_id.to_string(),
        parameters,
        request_body: request_body.map(|required| RequestBody {
            required,
            content: BTreeMap::from([(
                "application/json".to_string(),
                MediaType {
                    schema: schema_for!(serde_json::Value).schema.into(),
                },
            )]),
        }),
        responses: BTreeMap::from([(
            "200".to_string(),
            Response {
                description: response_description.to_string(),
                content: BTreeMap::from([(
                    "application/json".to_string(),
                    MediaType {
                        schema: schema_for!(serde_json::Value).schema.into(),
                    },
                )]),
            },
        )]),
        security: vec![SecurityRequirement {
            scheme: BTreeMap::from([("Bearer".to_string(), vec!["read".to_string()])]),
        }],
    }
}

fn query_param(name: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        r#in: "query".to_string(),
        required: false,
        schema: schema_for!(String).schema.into(),
    }
}

fn path_param(name: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        r#in: "path".to_string(),
        required: true,
        schema: schema_for!(String).schema.into(),
    }
}

/// OpenAPI components (schemas, security schemes, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    pub schemas: BTreeMap<String, Schema>,
    pub security_schemes: BTreeMap<String, SecurityScheme>,
}

impl OpenApiDocument {
    /// Generate the complete OpenAPI 3.1 document for the Vyapaar REST API.
    pub fn generate() -> Self {
        let mut paths = BTreeMap::new();

        // Market data endpoints
        paths.insert(
            "/instruments".to_string(),
            PathItem {
                get: Some(Operation {
                    tags: vec!["market".to_string()],
                    summary: "List all instruments".to_string(),
                    operation_id: "getInstruments".to_string(),
                    parameters: vec![],
                    request_body: None,
                    responses: BTreeMap::from([(
                        "200".to_string(),
                        Response {
                            description: "List of instruments".to_string(),
                            content: BTreeMap::from([(
                                "application/json".to_string(),
                                MediaType {
                                    schema: schema_for!(Vec<InstrumentId>).schema.into(),
                                },
                            )]),
                        },
                    )]),
                    security: vec![SecurityRequirement {
                        scheme: BTreeMap::from([("Bearer".to_string(), vec!["read".to_string()])]),
                    }],
                }),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );

        paths.insert(
            "/quotes".to_string(),
            PathItem {
                get: Some(Operation {
                    tags: vec!["market".to_string()],
                    summary: "Get latest quotes".to_string(),
                    operation_id: "getQuotes".to_string(),
                    parameters: vec![],
                    request_body: None,
                    responses: BTreeMap::from([(
                        "200".to_string(),
                        Response {
                            description: "Latest quotes".to_string(),
                            content: BTreeMap::from([(
                                "application/json".to_string(),
                                MediaType {
                                    schema: schema_for!(serde_json::Value).schema.into(),
                                },
                            )]),
                        },
                    )]),
                    security: vec![SecurityRequirement {
                        scheme: BTreeMap::from([("Bearer".to_string(), vec!["read".to_string()])]),
                    }],
                }),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );

        paths.insert(
            "/orders".to_string(),
            PathItem {
                get: Some(Operation {
                    tags: vec!["trading".to_string()],
                    summary: "List orders".to_string(),
                    operation_id: "getOrders".to_string(),
                    parameters: vec![],
                    request_body: None,
                    responses: BTreeMap::from([(
                        "200".to_string(),
                        Response {
                            description: "List of orders".to_string(),
                            content: BTreeMap::from([(
                                "application/json".to_string(),
                                MediaType {
                                    schema: schema_for!(serde_json::Value).schema.into(),
                                },
                            )]),
                        },
                    )]),
                    security: vec![SecurityRequirement {
                        scheme: BTreeMap::from([("Bearer".to_string(), vec!["trade".to_string()])]),
                    }],
                }),
                post: Some(Operation {
                    tags: vec!["trading".to_string()],
                    summary: "Place a new order".to_string(),
                    operation_id: "postOrders".to_string(),
                    parameters: vec![],
                    request_body: Some(RequestBody {
                        required: true,
                        content: BTreeMap::from([(
                            "application/json".to_string(),
                            MediaType {
                                schema: schema_for!(serde_json::Value).schema.into(),
                            },
                        )]),
                    }),
                    responses: BTreeMap::from([
                        (
                            "200".to_string(),
                            Response {
                                description: "Order placed".to_string(),
                                content: BTreeMap::from([(
                                    "application/json".to_string(),
                                    MediaType {
                                        schema: schema_for!(serde_json::Value).schema.into(),
                                    },
                                )]),
                            },
                        ),
                        (
                            "422".to_string(),
                            Response {
                                description: "Risk refusal or validation error".to_string(),
                                content: BTreeMap::from([(
                                    "application/json".to_string(),
                                    MediaType {
                                        schema: schema_for!(serde_json::Value).schema.into(),
                                    },
                                )]),
                            },
                        ),
                    ]),
                    security: vec![SecurityRequirement {
                        scheme: BTreeMap::from([("Bearer".to_string(), vec!["trade".to_string()])]),
                    }],
                }),
                put: None,
                patch: None,
                delete: None,
            },
        );

        paths.insert(
            "/orders/{id}".to_string(),
            PathItem {
                get: None,
                post: None,
                put: None,
                patch: None,
                delete: Some(Operation {
                    tags: vec!["trading".to_string()],
                    summary: "Cancel an order".to_string(),
                    operation_id: "deleteOrder".to_string(),
                    parameters: vec![Parameter {
                        name: "id".to_string(),
                        r#in: "path".to_string(),
                        required: true,
                        schema: schema_for!(String).schema.into(),
                    }],
                    request_body: None,
                    responses: BTreeMap::from([(
                        "200".to_string(),
                        Response {
                            description: "Order cancelled".to_string(),
                            content: BTreeMap::from([(
                                "application/json".to_string(),
                                MediaType {
                                    schema: schema_for!(serde_json::Value).schema.into(),
                                },
                            )]),
                        },
                    )]),
                    security: vec![SecurityRequirement {
                        scheme: BTreeMap::from([("Bearer".to_string(), vec!["trade".to_string()])]),
                    }],
                }),
            },
        );

        paths.insert(
            "/portfolio".to_string(),
            PathItem {
                get: Some(Operation {
                    tags: vec!["portfolio".to_string()],
                    summary: "Get portfolio".to_string(),
                    operation_id: "getPortfolio".to_string(),
                    parameters: vec![],
                    request_body: None,
                    responses: BTreeMap::from([(
                        "200".to_string(),
                        Response {
                            description: "Portfolio state".to_string(),
                            content: BTreeMap::from([(
                                "application/json".to_string(),
                                MediaType {
                                    schema: schema_for!(serde_json::Value).schema.into(),
                                },
                            )]),
                        },
                    )]),
                    security: vec![SecurityRequirement {
                        scheme: BTreeMap::from([("Bearer".to_string(), vec!["read".to_string()])]),
                    }],
                }),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );

        paths.insert(
            "/schema".to_string(),
            PathItem {
                get: Some(Operation {
                    tags: vec!["meta".to_string()],
                    summary: "Get OpenAPI schema".to_string(),
                    operation_id: "getSchema".to_string(),
                    parameters: vec![],
                    request_body: None,
                    responses: BTreeMap::from([(
                        "200".to_string(),
                        Response {
                            description: "OpenAPI 3.1 schema".to_string(),
                            content: BTreeMap::from([(
                                "application/json".to_string(),
                                MediaType {
                                    schema: schema_for!(serde_json::Value).schema.into(),
                                },
                            )]),
                        },
                    )]),
                    security: vec![],
                }),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );

        // Trade-performance journal endpoints (/trade-journal/*), backed by
        // vyapaar-ledger-performance. Journal accounts are distinct from
        // /portfolio/accounts (live OMS/trading accounts).
        paths.insert(
            "/trade-journal/accounts".to_string(),
            PathItem {
                get: Some(operation(
                    "trade-journal",
                    "List journal accounts",
                    "listTradeJournalAccounts",
                    vec![],
                    None,
                    "List of journal accounts",
                )),
                post: Some(operation(
                    "trade-journal",
                    "Create a journal account",
                    "createTradeJournalAccount",
                    vec![],
                    Some(true),
                    "Created journal account",
                )),
                put: None,
                patch: None,
                delete: None,
            },
        );
        paths.insert(
            "/trade-journal/overview".to_string(),
            PathItem {
                get: Some(operation(
                    "trade-journal",
                    "Performance metrics and edge score",
                    "getTradeJournalOverview",
                    vec![
                        query_param("from"),
                        query_param("to"),
                        query_param("compare"),
                    ],
                    None,
                    "Overview metrics and edge score",
                )),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );
        paths.insert(
            "/trade-journal/trades".to_string(),
            PathItem {
                get: Some(operation(
                    "trade-journal",
                    "List closed trades with annotations",
                    "listTradeJournalTrades",
                    vec![
                        query_param("from"),
                        query_param("to"),
                        query_param("limit"),
                        query_param("cursor"),
                    ],
                    None,
                    "Cursor-paginated closed trades",
                )),
                post: None,
                put: None,
                delete: None,
                patch: Some(operation(
                    "trade-journal",
                    "Set a trade's annotation",
                    "patchTradeJournalTrade",
                    vec![path_param("id")],
                    Some(true),
                    "Updated trade annotation",
                )),
            },
        );
        paths.insert(
            "/trade-journal/calendar".to_string(),
            PathItem {
                get: Some(operation(
                    "trade-journal",
                    "Daily P&L heatmap for a month",
                    "getTradeJournalCalendar",
                    vec![query_param("month")],
                    None,
                    "Monthly P&L calendar",
                )),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );
        paths.insert(
            "/trade-journal/notes".to_string(),
            PathItem {
                get: Some(operation(
                    "trade-journal",
                    "List day-keyed journal notes",
                    "listTradeJournalNotes",
                    vec![query_param("account"), query_param("limit")],
                    None,
                    "Journal notes",
                )),
                post: Some(operation(
                    "trade-journal",
                    "Create or replace a journal note",
                    "createTradeJournalNote",
                    vec![],
                    Some(true),
                    "Created journal note",
                )),
                put: None,
                patch: None,
                delete: None,
            },
        );
        paths.insert(
            "/trade-journal/compare".to_string(),
            PathItem {
                get: Some(operation(
                    "trade-journal",
                    "Compare two performances",
                    "compareTradeJournalAccounts",
                    vec![
                        query_param("accountA"),
                        query_param("accountB"),
                        query_param("account"),
                        query_param("mode"),
                    ],
                    None,
                    "Two-sided metrics comparison",
                )),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );

        // Market reference data endpoints (/market-data/*), backed by
        // vyapaar-data's calendar and symbol-profile ingestion.
        paths.insert(
            "/market-data/symbol-profile/{id}".to_string(),
            PathItem {
                get: Some(operation(
                    "market-reference",
                    "Company symbol profile / fundamentals",
                    "getSymbolProfile",
                    vec![path_param("id")],
                    None,
                    "Symbol profile",
                )),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );
        paths.insert(
            "/market-data/calendar".to_string(),
            PathItem {
                get: Some(operation(
                    "market-reference",
                    "Economic / earnings / dividends / splits / IPO calendar",
                    "getMarketCalendar",
                    vec![
                        query_param("from"),
                        query_param("to"),
                        query_param("category"),
                    ],
                    None,
                    "Calendar events",
                )),
                post: None,
                put: None,
                patch: None,
                delete: None,
            },
        );

        let mut schemas = BTreeMap::new();
        schemas.insert(
            "InstrumentId".to_string(),
            schema_for!(InstrumentId).schema.into(),
        );
        schemas.insert(
            "OrderSide".to_string(),
            schema_for!(OrderSide).schema.into(),
        );
        schemas.insert(
            "OrderStatus".to_string(),
            schema_for!(OrderStatus).schema.into(),
        );
        schemas.insert(
            "OrderType".to_string(),
            schema_for!(OrderType).schema.into(),
        );
        schemas.insert("Price".to_string(), schema_for!(Price).schema.into());
        schemas.insert("Quantity".to_string(), schema_for!(Quantity).schema.into());
        schemas.insert(
            "TimeInForce".to_string(),
            schema_for!(TimeInForce).schema.into(),
        );
        schemas.insert(
            "RiskLimits".to_string(),
            schema_for!(RiskLimits).schema.into(),
        );
        schemas.insert(
            "RiskRefusal".to_string(),
            schema_for!(RiskRefusal).schema.into(),
        );

        let mut security_schemes = BTreeMap::new();
        security_schemes.insert(
            "Bearer".to_string(),
            SecurityScheme::Http {
                scheme: "bearer".to_string(),
                bearer_format: Some("JWT".to_string()),
            },
        );

        Self {
            openapi: "3.1.0".to_string(),
            info: OpenApiInfo {
                title: "Vyapaar REST API".to_string(),
                version: vyapaar_runtime_api::API_VERSION.to_string(),
                description:
                    "Vyapaar trading platform REST API with JWT authentication and OpenAPI 3.1"
                        .to_string(),
            },
            servers: vec![OpenApiServer {
                url: "http://localhost:8080".to_string(),
                description: "Vyapaar API server".to_string(),
            }],
            paths,
            components: Components {
                schemas,
                security_schemes,
            },
            tags: vec![
                Tag {
                    name: "market".to_string(),
                    description: "Market data endpoints (instruments, quotes, bars, depth)"
                        .to_string(),
                },
                Tag {
                    name: "trading".to_string(),
                    description: "Trading endpoints (orders, positions)".to_string(),
                },
                Tag {
                    name: "portfolio".to_string(),
                    description: "Portfolio endpoints (holdings, accounts, trades)".to_string(),
                },
                Tag {
                    name: "meta".to_string(),
                    description: "Meta endpoints (schema, health, capabilities)".to_string(),
                },
                Tag {
                    name: "trade-journal".to_string(),
                    description:
                        "Trade-performance journal (annotations, metrics, calendar, comparison)"
                            .to_string(),
                },
                Tag {
                    name: "market-reference".to_string(),
                    description: "Market reference data (symbol profile, economic calendar)"
                        .to_string(),
                },
            ],
        }
    }

    /// Serialize the generated OpenAPI 3.1 document as a JSON `Value`,
    /// suitable for writing to a file (`vyapaar schema --openapi`).
    pub fn openapi_json() -> serde_json::Value {
        serde_json::to_value(Self::generate()).unwrap_or(serde_json::Value::Null)
    }

    /// Generate the OpenAPI 3.1 document for v1 (legacy & supported).
    pub fn generate_v1() -> Self {
        let mut doc = Self::generate();
        doc.info.title = "Vyapaar REST API v1".to_string();
        doc.info.version = "1.0.0".to_string();
        doc.info.description =
            "Vyapaar trading platform REST API v1 (Supported for legacy software)".to_string();
        doc.servers = vec![
            OpenApiServer {
                url: "/api/v1".to_string(),
                description: "Vyapaar REST API v1 (Supported for legacy software)".to_string(),
            },
            OpenApiServer {
                url: "/".to_string(),
                description: "Legacy un-prefixed root (Deprecated)".to_string(),
            },
        ];
        doc
    }

    /// Generate the OpenAPI 3.1 document for v2 (latest & recommended).
    pub fn generate_v2() -> Self {
        let mut doc = Self::generate();
        doc.info.title = "Vyapaar REST API v2".to_string();
        doc.info.version = "2.0.0".to_string();
        doc.info.description =
            "Vyapaar trading platform REST API v2 (Latest & Recommended)".to_string();
        doc.servers = vec![OpenApiServer {
            url: "/api/v2".to_string(),
            description: "Vyapaar REST API v2 (Latest & Recommended)".to_string(),
        }];
        doc
    }
}

/// Handler for `GET /schema/openapi/v1.json`.
pub async fn get_openapi_v1() -> axum::response::Response {
    let doc = OpenApiDocument::generate_v1();
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    )
        .into_response()
}

/// Handler for `GET /schema/openapi/v2.json`.
pub async fn get_openapi_v2() -> axum::response::Response {
    let doc = OpenApiDocument::generate_v2();
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    )
        .into_response()
}

/// Handler for `GET /schema/openapi`.
pub async fn get_openapi_index() -> axum::response::Response {
    let index = serde_json::json!({
        "openapi_versions": [
            {
                "version": "v1",
                "status": "supported_legacy",
                "url": "/schema/openapi/v1.json",
                "description": "Vyapaar REST API v1 (Supported for legacy software)"
            },
            {
                "version": "v2",
                "status": "latest_recommended",
                "url": "/schema/openapi/v2.json",
                "description": "Vyapaar REST API v2 (Latest & Recommended)"
            }
        ],
        "latest": "v2",
        "documentation_url": "/docs"
    });
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string_pretty(&index).unwrap_or_default(),
    )
        .into_response()
}

/// Handler for `GET /docs` and `GET /swagger-ui`.
pub async fn get_swagger_ui() -> axum::response::Html<&'static str> {
    axum::response::Html(SWAGGER_UI_HTML)
}

/// Swagger UI HTML page with multi-version selector.
pub const SWAGGER_UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>Vyapaar REST API Documentation</title>
  <link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css" />
  <style>
    body { margin: 0; background: #fafafa; font-family: sans-serif; }
    .topbar { display: block; }
  </style>
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
  <script>
    window.onload = function() {
      window.ui = SwaggerUIBundle({
        urls: [
          { url: "/schema/openapi/v2.json", name: "v2 (Latest & Recommended)" },
          { url: "/schema/openapi/v1.json", name: "v1 (Supported / Legacy)" }
        ],
        "urls.primaryName": "v2 (Latest & Recommended)",
        dom_id: '#swagger-ui',
        deepLinking: true,
        presets: [
          SwaggerUIBundle.presets.apis,
          SwaggerUIStandalonePreset
        ],
        layout: "StandaloneLayout"
      });
    };
  </script>
</body>
</html>"#;
