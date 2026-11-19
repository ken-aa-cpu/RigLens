//! VTube Studio Public API v1.0 WebSocket client (`ws://127.0.0.1:8001`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::physics::{BoosterPhysicsEngine, MotionPoseInput};

const VTS_API_NAME: &str = "VTubeStudioPublicAPI";
const VTS_API_VERSION: &str = "1.0";

#[derive(Debug, Error)]
pub enum VtsError {
	#[error("WebSocket error: {0}")]
	WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
	#[error("JSON serialization error: {0}")]
	Json(#[from] serde_json::Error),
	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),
	#[error("VTube Studio API error (id={error_id}): {message}")]
	Api { error_id: i32, message: String },
	#[error("Request '{0}' timed out")]
	Timeout(String),
	#[error("Unexpected response message type: expected '{expected}', got '{actual}'")]
	UnexpectedResponse { expected: String, actual: String },
	#[error("Connection closed unexpectedly")]
	Closed,
	#[error("Invalid custom parameter name '{0}': must be 4..=32 ASCII alphanumeric chars")]
	InvalidParameterName(String),
	#[error("Authentication rejected by user or VTube Studio: {0}")]
	AuthRejected(String),
}

#[derive(Clone, Debug)]
pub struct VtsClientConfig {
	pub url: String,
	pub plugin_name: String,
	pub plugin_developer: String,
	pub plugin_icon: Option<String>,
	pub token_path: Option<PathBuf>,
	pub request_timeout: Duration,
	pub auth_timeout: Duration,
}

impl Default for VtsClientConfig {
	fn default() -> Self {
		Self {
			url: "ws://127.0.0.1:8001".to_owned(),
			plugin_name: "RigLens Booster".to_owned(),
			plugin_developer: "RigLens".to_owned(),
			plugin_icon: None,
			token_path: None,
			request_timeout: Duration::from_secs(5),
			auth_timeout: Duration::from_secs(45),
		}
	}
}

/// Default local file for caching VTube Studio plugin auth tokens so users only approve once.
pub fn default_token_path() -> PathBuf {
	if let Ok(appdata) = std::env::var("APPDATA") {
		PathBuf::from(appdata).join("RigLens").join("vts_token.txt")
	} else if let Ok(home) = std::env::var("HOME") {
		PathBuf::from(home).join(".config").join("RigLens").join("vts_token.txt")
	} else {
		std::env::temp_dir().join("riglens_vts_token.txt")
	}
}

/// Validate VTS custom parameter constraints: unique alphanumeric, 4..=32 characters, no spaces.
pub fn validate_custom_param_name(name: &str) -> Result<(), VtsError> {
	if (4..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric()) {
		Ok(())
	} else {
		Err(VtsError::InvalidParameterName(name.to_owned()))
	}
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VtsEnvelopeOut<T: Serialize> {
	api_name: &'static str,
	api_version: &'static str,
	#[serde(rename = "requestID")]
	request_id: String,
	message_type: &'static str,
	data: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VtsEnvelopeIn {
	#[serde(default, rename = "requestID")]
	request_id: String,
	message_type: String,
	#[serde(default)]
	data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiErrorData {
	#[serde(default, rename = "errorID")]
	error_id: i32,
	#[serde(default)]
	message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthTokenReq {
	plugin_name: String,
	plugin_developer: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	plugin_icon: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthTokenResp {
	authentication_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthReq {
	plugin_name: String,
	plugin_developer: String,
	authentication_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthResp {
	authenticated: bool,
	#[serde(default)]
	reason: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VtsModelPosition {
	#[serde(default)]
	pub position_x: f32,
	#[serde(default)]
	pub position_y: f32,
	#[serde(default)]
	pub rotation: f32,
	#[serde(default)]
	pub size: f32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentModelResponse {
	#[serde(default)]
	pub model_loaded: bool,
	#[serde(default)]
	pub model_name: String,
	#[serde(default, rename = "modelID")]
	pub model_id: String,
	#[serde(default)]
	pub model_position: VtsModelPosition,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VtsInputParam {
	#[serde(default)]
	pub name: String,
	#[serde(default)]
	pub added_by: String,
	#[serde(default)]
	pub value: f32,
	#[serde(default)]
	pub min: f32,
	#[serde(default)]
	pub max: f32,
	#[serde(default)]
	pub default_value: f32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputParameterListResponse {
	#[serde(default)]
	pub model_loaded: bool,
	#[serde(default)]
	pub model_name: String,
	#[serde(default, rename = "modelID")]
	pub model_id: String,
	#[serde(default)]
	pub custom_parameters: Vec<VtsInputParam>,
	#[serde(default)]
	pub default_parameters: Vec<VtsInputParam>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ParameterCreationReq<'a> {
	parameter_name: &'a str,
	explanation: &'a str,
	min: f64,
	max: f64,
	default_value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VtsParamValue {
	pub id: String,
	pub value: f32,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub weight: Option<f32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InjectParameterDataReq<'a> {
	face_found: bool,
	mode: &'a str,
	parameter_values: Vec<VtsParamValue>,
}

/// Bidirectional async client for VTube Studio Public API v1.0.
pub struct VtsClient {
	config: VtsClientConfig,
	ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
	next_req_id: AtomicU64,
	token: Option<String>,
}

impl VtsClient {
	/// Connect to VTube Studio's WebSocket server (`ws://127.0.0.1:8001` by default).
	pub async fn connect(config: VtsClientConfig) -> Result<Self, VtsError> {
		let (ws, _) = connect_async(&config.url).await?;
		Ok(Self {
			config,
			ws,
			next_req_id: AtomicU64::new(1),
			token: None,
		})
	}

	fn token_file(&self) -> PathBuf {
		self.config
			.token_path
			.clone()
			.unwrap_or_else(default_token_path)
	}

	fn load_cached_token(path: &Path) -> Option<String> {
		let raw = std::fs::read_to_string(path).ok()?;
		let trimmed = raw.trim();
		if trimmed.is_empty() {
			None
		} else {
			Some(trimmed.to_owned())
		}
	}

	fn save_cached_token(path: &Path, token: &str) -> Result<(), std::io::Error> {
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)?;
		}
		std::fs::write(path, token)
	}

	async fn send_request<Req: Serialize, Resp: for<'de> Deserialize<'de>>(
		&mut self,
		message_type: &'static str,
		expected_response_type: &str,
		data: Req,
		timeout: Duration,
	) -> Result<Resp, VtsError> {
		let req_num = self.next_req_id.fetch_add(1, Ordering::Relaxed);
		let req_id = format!("riglens_{req_num}");
		let envelope = VtsEnvelopeOut {
			api_name: VTS_API_NAME,
			api_version: VTS_API_VERSION,
			request_id: req_id.clone(),
			message_type,
			data,
		};
		let payload = serde_json::to_string(&envelope)?;
		self.ws.send(Message::Text(payload.into())).await?;

		let deadline = tokio::time::timeout(timeout, async {
			while let Some(msg) = self.ws.next().await {
				let msg = msg?;
				match msg {
					Message::Text(txt) => {
						let resp: VtsEnvelopeIn = serde_json::from_str(txt.as_str())?;
						if !resp.request_id.is_empty() && resp.request_id != req_id {
							continue;
						}
						if resp.message_type == "APIError" {
							let err: ApiErrorData = serde_json::from_value(resp.data)?;
							return Err(VtsError::Api {
								error_id: err.error_id,
								message: err.message,
							});
						}
						if resp.message_type != expected_response_type {
							return Err(VtsError::UnexpectedResponse {
								expected: expected_response_type.to_owned(),
								actual: resp.message_type,
							});
						}
						let parsed: Resp = serde_json::from_value(resp.data)?;
						return Ok(parsed);
					}
					Message::Ping(bytes) => {
						self.ws.send(Message::Pong(bytes)).await?;
					}
					Message::Close(_) => return Err(VtsError::Closed),
					_ => {}
				}
			}
			Err(VtsError::Closed)
		})
		.await;

		match deadline {
			Ok(inner) => inner,
			Err(_) => Err(VtsError::Timeout(message_type.to_owned())),
		}
	}

	/// Authenticate with VTube Studio, reusing cached token if valid or requesting user approval once.
	pub async fn authenticate(&mut self) -> Result<String, VtsError> {
		let token_path = self.token_file();
		let cached = self
			.token
			.clone()
			.or_else(|| Self::load_cached_token(&token_path));

		if let Some(tok) = cached {
			let auth: AuthResp = self
				.send_request(
					"AuthenticationRequest",
					"AuthenticationResponse",
					AuthReq {
						plugin_name: self.config.plugin_name.clone(),
						plugin_developer: self.config.plugin_developer.clone(),
						authentication_token: tok.clone(),
					},
					self.config.request_timeout,
				)
				.await?;
			if auth.authenticated {
				self.token = Some(tok.clone());
				return Ok(tok);
			}
		}

		// Cached token missing or expired -> request new token via VTS prompt
		let token_resp: AuthTokenResp = self
			.send_request(
				"AuthenticationTokenRequest",
				"AuthenticationTokenResponse",
				AuthTokenReq {
					plugin_name: self.config.plugin_name.clone(),
					plugin_developer: self.config.plugin_developer.clone(),
					plugin_icon: self.config.plugin_icon.clone(),
				},
				self.config.auth_timeout,
			)
			.await?;
		let new_token = token_resp.authentication_token;

		let auth: AuthResp = self
			.send_request(
				"AuthenticationRequest",
				"AuthenticationResponse",
				AuthReq {
					plugin_name: self.config.plugin_name.clone(),
					plugin_developer: self.config.plugin_developer.clone(),
					authentication_token: new_token.clone(),
				},
				self.config.request_timeout,
			)
			.await?;

		if !auth.authenticated {
			return Err(VtsError::AuthRejected(auth.reason));
		}

		let _ = Self::save_cached_token(&token_path, &new_token);
		self.token = Some(new_token.clone());
		Ok(new_token)
	}

	/// Create or ensure a single custom VTube Studio tracking parameter exists.
	pub async fn create_parameter(
		&mut self,
		name: &str,
		explanation: &str,
		min: f64,
		max: f64,
		default_value: f64,
	) -> Result<(), VtsError> {
		validate_custom_param_name(name)?;
		let res: Result<serde_json::Value, VtsError> = self
			.send_request(
				"ParameterCreationRequest",
				"ParameterCreationResponse",
				ParameterCreationReq {
					parameter_name: name,
					explanation,
					min,
					max,
					default_value,
				},
				self.config.request_timeout,
			)
			.await;

		match res {
			Ok(_) => Ok(()),
			Err(VtsError::Api { error_id, message })
				if error_id == 352
					|| error_id == 353
					|| message.to_ascii_lowercase().contains("already") =>
			{
				Ok(())
			}
			Err(e) => Err(e),
		}
	}

	/// Register all 9 `RigLens*` physics parameters in VTube Studio.
	pub async fn register_booster_parameters(&mut self) -> Result<Vec<String>, VtsError> {
		let mut created = Vec::new();
		for spec in BoosterPhysicsEngine::parameter_specs() {
			self.create_parameter(
				spec.name,
				spec.explanation,
				spec.min,
				spec.max,
				spec.default_value,
			)
			.await?;
			created.push(spec.name.to_owned());
		}
		Ok(created)
	}

	/// Query info and screen transform of the currently active model in VTube Studio.
	pub async fn get_current_model(&mut self) -> Result<CurrentModelResponse, VtsError> {
		self.send_request(
			"CurrentModelRequest",
			"CurrentModelResponse",
			serde_json::json!({}),
			self.config.request_timeout,
		)
		.await
	}

	/// Query all default and custom tracking input parameters currently present in VTube Studio.
	pub async fn get_input_parameters(&mut self) -> Result<InputParameterListResponse, VtsError> {
		self.send_request(
			"InputParameterListRequest",
			"InputParameterListResponse",
			serde_json::json!({}),
			self.config.request_timeout,
		)
		.await
	}

	/// Poll current VTube Studio tracking inputs into a normalized [`MotionPoseInput`].
	pub async fn poll_motion_pose(&mut self, audio_rms: f32) -> Result<MotionPoseInput, VtsError> {
		let list = self.get_input_parameters().await?;
		let mut map: HashMap<&str, f32> = HashMap::new();
		for p in &list.default_parameters {
			map.insert(p.name.as_str(), p.value);
		}
		for p in &list.custom_parameters {
			map.entry(p.name.as_str()).or_insert(p.value);
		}

		let get = |names: &[&str], fallback: f32| -> f32 {
			for &n in names {
				if let Some(&v) = map.get(n) {
					if v.is_finite() {
						return v;
					}
				}
			}
			fallback
		};

		Ok(MotionPoseInput {
			face_found: list.model_loaded && !list.default_parameters.is_empty(),
			head_pos: Vec2::new(
				get(&["FacePositionX"], 0.0),
				get(&["FacePositionY"], 0.0),
			),
			head_angle: Vec3::new(
				get(&["FaceAngleX", "ParamAngleX"], 0.0),
				get(&["FaceAngleY", "ParamAngleY"], 0.0),
				get(&["FaceAngleZ", "ParamAngleZ"], 0.0),
			),
			body_angle: Vec3::new(
				get(&["BodyAngleX", "ParamBodyAngleX"], 0.0),
				get(&["BodyAngleY", "ParamBodyAngleY"], 0.0),
				get(&["BodyAngleZ", "ParamBodyAngleZ"], 0.0),
			),
			eye_open_l: get(&["EyeOpenLeft", "ParamEyeLOpen"], 1.0),
			eye_open_r: get(&["EyeOpenRight", "ParamEyeROpen"], 1.0),
			mouth_open: get(&["MouthOpen", "ParamMouthOpenY"], 0.0),
			audio_rms,
		})
	}

	/// Inject parameter values (`mode = "set"` or `"add"`) into VTube Studio.
	pub async fn inject_parameters(
		&mut self,
		face_found: bool,
		mode: &str,
		values: &[(String, f32)],
	) -> Result<(), VtsError> {
		let parameter_values = values
			.iter()
			.filter(|(_, v)| v.is_finite())
			.map(|(id, value)| VtsParamValue {
				id: id.clone(),
				value: *value,
				weight: None,
			})
			.collect();
		let _: serde_json::Value = self
			.send_request(
				"InjectParameterDataRequest",
				"InjectParameterDataResponse",
				InjectParameterDataReq {
					face_found,
					mode,
					parameter_values,
				},
				self.config.request_timeout,
			)
			.await?;
		Ok(())
	}

	pub async fn close(mut self) -> Result<(), VtsError> {
		self.ws.close(None).await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Arc;
	use tokio::net::TcpListener;
	use tokio::sync::Mutex;
	use tokio_tungstenite::accept_async;

	#[derive(Default)]
	struct MockVtsState {
		created_params: Vec<String>,
		injected_batches: Vec<Vec<VtsParamValue>>,
		auth_token_requests: usize,
	}

	#[tokio::test]
	async fn mock_vts_auth_token_cache_create_poll_and_inject() {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let state = Arc::new(Mutex::new(MockVtsState::default()));
		let server_state = state.clone();

		let server_task = tokio::spawn(async move {
			for _conn in 0..2 {
				let (stream, _) = listener.accept().await.unwrap();
				let st = server_state.clone();
				tokio::spawn(async move {
					let mut ws = accept_async(stream).await.unwrap();
					let expected_token = "tok_riglens_secret_123";
					while let Some(Ok(Message::Text(txt))) = ws.next().await {
						let req: VtsEnvelopeIn = serde_json::from_str(txt.as_str()).unwrap();
						let mut s = st.lock().await;
						let (resp_type, data) = match req.message_type.as_str() {
							"AuthenticationTokenRequest" => {
								s.auth_token_requests += 1;
								(
									"AuthenticationTokenResponse",
									serde_json::json!({ "authenticationToken": expected_token }),
								)
							}
							"AuthenticationRequest" => {
								let tok = req.data["authenticationToken"].as_str().unwrap_or("");
								let ok = tok == expected_token;
								(
									"AuthenticationResponse",
									serde_json::json!({
										"authenticated": ok,
										"reason": if ok { "OK" } else { "Bad token" }
									}),
								)
							}
							"ParameterCreationRequest" => {
								let name = req.data["parameterName"].as_str().unwrap().to_owned();
								s.created_params.push(name.clone());
								("ParameterCreationResponse", serde_json::json!({ "parameterName": name }))
							}
							"CurrentModelRequest" => (
								"CurrentModelResponse",
								serde_json::json!({
									"modelLoaded": true,
									"modelName": "SampleVTSModel",
									"modelID": "model_01",
									"modelPosition": { "positionX": 0.1, "positionY": -0.2, "rotation": 5.0, "size": -10.0 }
								}),
							),
							"InputParameterListRequest" => (
								"InputParameterListResponse",
								serde_json::json!({
									"modelLoaded": true,
									"modelName": "SampleVTSModel",
									"modelID": "model_01",
									"defaultParameters": [
										{ "name": "FaceAngleX", "value": 12.5, "min": -30.0, "max": 30.0, "defaultValue": 0.0 },
										{ "name": "EyeOpenLeft", "value": 0.25, "min": 0.0, "max": 1.0, "defaultValue": 1.0 }
									],
									"customParameters": []
								}),
							),
							"InjectParameterDataRequest" => {
								let vals: Vec<VtsParamValue> =
									serde_json::from_value(req.data["parameterValues"].clone()).unwrap();
								s.injected_batches.push(vals);
								("InjectParameterDataResponse", serde_json::json!({}))
							}
							other => ("APIError", serde_json::json!({ "errorID": 1, "message": format!("Unknown {other}") })),
						};
						let reply = serde_json::json!({
							"apiName": VTS_API_NAME,
							"apiVersion": VTS_API_VERSION,
							"timestamp": 1,
							"requestID": req.request_id,
							"messageType": resp_type,
							"data": data,
						});
						ws.send(Message::Text(reply.to_string().into())).await.unwrap();
					}
				});
			}
		});

		let token_dir = std::env::temp_dir().join("riglens_booster_vts_test");
		let _ = std::fs::create_dir_all(&token_dir);
		let token_path = token_dir.join("vts_token.txt");
		let _ = std::fs::remove_file(&token_path);

		let cfg = VtsClientConfig {
			url: format!("ws://{addr}"),
			token_path: Some(token_path.clone()),
			request_timeout: Duration::from_secs(3),
			auth_timeout: Duration::from_secs(3),
			..Default::default()
		};

		// First session: no cached token -> AuthenticationTokenRequest + save to disk + register + poll + inject
		let mut client = VtsClient::connect(cfg.clone()).await.unwrap();
		let tok = client.authenticate().await.unwrap();
		assert_eq!(tok, "tok_riglens_secret_123");
		assert_eq!(std::fs::read_to_string(&token_path).unwrap().trim(), tok);

		let created = client.register_booster_parameters().await.unwrap();
		assert_eq!(created.len(), 9);

		let model = client.get_current_model().await.unwrap();
		assert!(model.model_loaded);
		assert_eq!(model.model_name, "SampleVTSModel");

		let pose = client.poll_motion_pose(0.4).await.unwrap();
		assert!(pose.face_found);
		assert!((pose.head_angle.x - 12.5).abs() < 1e-4);
		assert!((pose.eye_open_l - 0.25).abs() < 1e-4);

		let mut engine = BoosterPhysicsEngine::default();
		let phys_out = engine.step(&pose, 1.0 / 120.0);
		client
			.inject_parameters(true, "set", &phys_out.to_vts_parameters())
			.await
			.unwrap();
		client.close().await.unwrap();

		// Second session: cached token must avoid another AuthenticationTokenRequest
		let mut client2 = VtsClient::connect(cfg).await.unwrap();
		let tok2 = client2.authenticate().await.unwrap();
		assert_eq!(tok2, "tok_riglens_secret_123");
		client2.close().await.unwrap();

		let st = state.lock().await;
		assert_eq!(st.auth_token_requests, 1, "expected token reuse on second connect");
		assert_eq!(st.created_params.len(), 9);
		assert_eq!(st.injected_batches.len(), 1);
		assert_eq!(st.injected_batches[0].len(), 9);

		server_task.abort();
		let _ = std::fs::remove_dir_all(&token_dir);
	}
}
