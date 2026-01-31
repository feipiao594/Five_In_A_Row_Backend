use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct EnvelopeIn {
  pub v: u32,
  #[serde(rename = "type")]
  pub r#type: String,
  #[serde(default, rename = "reqId")]
  pub req_id: Option<String>,
  #[serde(default)]
  pub ts: Option<i64>,
  #[serde(default)]
  pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct EnvelopeOut {
  pub v: u32,
  #[serde(rename = "type")]
  pub r#type: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  #[serde(rename = "reqId")]
  pub req_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub ok: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub error: Option<WsError>,
  pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct WsError {
  pub code: String,
  pub message: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub details: Option<serde_json::Value>,
}

impl EnvelopeOut {
  pub fn event(ty: &str, payload: serde_json::Value) -> Self {
    Self {
      v: 1,
      r#type: ty.to_string(),
      req_id: None,
      ok: None,
      error: None,
      payload,
    }
  }

  pub fn resp_ok(req: &EnvelopeIn, payload: serde_json::Value) -> Self {
    Self {
      v: 1,
      r#type: format!("{}.resp", req.r#type),
      req_id: req.req_id.clone(),
      ok: Some(true),
      error: None,
      payload,
    }
  }

  pub fn resp_err(req: &EnvelopeIn, code: &str, message: &str) -> Self {
    Self {
      v: 1,
      r#type: format!("{}.resp", req.r#type),
      req_id: req.req_id.clone(),
      ok: Some(false),
      error: Some(WsError {
        code: code.to_string(),
        message: message.to_string(),
        details: None,
      }),
      payload: serde_json::json!({}),
    }
  }
}
