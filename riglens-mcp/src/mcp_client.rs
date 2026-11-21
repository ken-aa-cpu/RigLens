use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use json::JsonValue;

/// Tool descriptor returned by a remote MCP server's `tools/list`.
#[derive(Clone, Debug, PartialEq)]
pub struct McpToolInfo {
	pub name: String,
	pub description: String,
	pub input_schema: JsonValue,
}

/// Companion Model Context Protocol (MCP) JSON-RPC 2.0 client over stdio streams.
///
/// Allows RigLens / `inox2d-mcp` to connect to external Inochi2D or DCC tools that
/// already expose a stdio MCP endpoint (for example, `nijigenerate`), perform the
/// `initialize` handshake, discover remote tools, and invoke them deterministically.
pub struct McpClient<R: BufRead, W: Write> {
	reader: R,
	writer: W,
	next_id: u64,
}

impl<R: BufRead, W: Write> McpClient<R, W> {
	pub fn new(reader: R, writer: W) -> Self {
		Self {
			reader,
			writer,
			next_id: 1,
		}
	}

	fn send_message(&mut self, msg: &JsonValue) -> Result<(), String> {
		let line = json::stringify(msg.clone());
		writeln!(self.writer, "{line}").map_err(|e| format!("MCP write failed: {e}"))?;
		self.writer.flush().map_err(|e| format!("MCP flush failed: {e}"))
	}

	/// Send a JSON-RPC 2.0 request and wait for its matching response `id`,
	/// ignoring any interleaved server notifications.
	pub fn request(&mut self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
		let id = self.next_id;
		self.next_id += 1;

		let req = json::object! {
			jsonrpc: "2.0",
			id: id,
			method: method,
			params: params
		};
		self.send_message(&req)?;

		let mut line = String::new();
		loop {
			line.clear();
			let n = self
				.reader
				.read_line(&mut line)
				.map_err(|e| format!("MCP read failed: {e}"))?;
			if n == 0 {
				return Err(format!("MCP server closed stream waiting for response to '{method}' (id={id})"));
			}
			let trimmed = line.trim();
			if trimmed.is_empty() {
				continue;
			}
			let msg = json::parse(trimmed).map_err(|e| format!("Invalid JSON from MCP server: {e}"))?;
			if msg["id"].as_u64() != Some(id) {
				// Skip asynchronous notifications or responses for other request ids.
				continue;
			}
			if !msg["error"].is_null() {
				let code = msg["error"]["code"].as_i64().unwrap_or(-32000);
				let message = msg["error"]["message"].as_str().unwrap_or("Unknown JSON-RPC error");
				return Err(format!("MCP error {code}: {message}"));
			}
			return Ok(msg["result"].clone());
		}
	}

	/// Perform MCP `initialize` + `notifications/initialized` handshake.
	pub fn initialize(&mut self, client_name: &str, client_version: &str) -> Result<JsonValue, String> {
		let result = self.request(
			"initialize",
			json::object! {
				protocolVersion: "2024-11-05",
				capabilities: {},
				clientInfo: {
					name: client_name,
					version: client_version
				}
			},
		)?;
		self.send_message(&json::object! {
			jsonrpc: "2.0",
			method: "notifications/initialized"
		})?;
		Ok(result)
	}

	/// Send MCP `ping` health-check request.
	pub fn ping(&mut self) -> Result<JsonValue, String> {
		self.request("ping", JsonValue::new_object())
	}

	/// Discover available tools exposed by the remote MCP server.
	pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, String> {
		let result = self.request("tools/list", JsonValue::new_object())?;
		let mut tools = Vec::new();
		for item in result["tools"].members() {
			tools.push(McpToolInfo {
				name: item["name"].as_str().unwrap_or_default().to_owned(),
				description: item["description"].as_str().unwrap_or_default().to_owned(),
				input_schema: item["inputSchema"].clone(),
			});
		}
		Ok(tools)
	}

	/// Discover readable resources exposed by the remote MCP server (if supported).
	pub fn list_resources(&mut self) -> Result<Vec<JsonValue>, String> {
		let result = self.request("resources/list", JsonValue::new_object())?;
		Ok(result["resources"].members().cloned().collect())
	}

	/// Invoke a remote MCP tool (`tools/call`) with JSON arguments.
	pub fn call_tool(&mut self, name: &str, arguments: JsonValue) -> Result<JsonValue, String> {
		let result = self.request(
			"tools/call",
			json::object! {
				name: name,
				arguments: arguments
			},
		)?;
		if result["isError"].as_bool() == Some(true) {
			let msg = result["content"][0]["text"]
				.as_str()
				.unwrap_or("Remote MCP tool reported error");
			return Err(msg.to_owned());
		}
		Ok(result)
	}
}

/// Child-process stdio wrapper for connecting to external MCP servers (e.g. `nijigenerate --mcp`).
pub struct McpStdioClient {
	child: Child,
	inner: McpClient<BufReader<ChildStdout>, ChildStdin>,
}

impl McpStdioClient {
	/// Spawn `program` with `args` and attach stdio pipes for JSON-RPC 2.0 MCP communication.
	pub fn spawn(program: &str, args: &[String]) -> Result<Self, String> {
		let mut child = Command::new(program)
			.args(args)
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.spawn()
			.map_err(|e| format!("Failed to spawn MCP server '{program}': {e}"))?;

		let stdin = child
			.stdin
			.take()
			.ok_or_else(|| "MCP child process missing stdin pipe".to_owned())?;
		let stdout = child
			.stdout
			.take()
			.ok_or_else(|| "MCP child process missing stdout pipe".to_owned())?;

		Ok(Self {
			child,
			inner: McpClient::new(BufReader::new(stdout), stdin),
		})
	}

	pub fn client_mut(&mut self) -> &mut McpClient<BufReader<ChildStdout>, ChildStdin> {
		&mut self.inner
	}
}

impl Drop for McpStdioClient {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

#[cfg(test)]
mod tests {
	use std::io::Cursor;

	use super::*;

	#[test]
	fn mcp_client_handshake_lists_and_calls_tools_skipping_notifications() {
		let server_lines = [
			r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"nijigenerate","version":"0.8.7"}}}"#,
			r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":0.5}}"#,
			r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ng_select_node","description":"Select node in nijigenerate","inputSchema":{"type":"object"}}]}}"#,
			r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"Selected Head"}]}}"#,
		]
		.join("\n")
			+ "\n";

		let reader = Cursor::new(server_lines.into_bytes());
		let writer = Vec::<u8>::new();
		let mut client = McpClient::new(reader, writer);

		let init = client.initialize("riglens-companion", "0.1.0").expect("initialize");
		assert_eq!(init["serverInfo"]["name"].as_str(), Some("nijigenerate"));

		let tools = client.list_tools().expect("list_tools");
		assert_eq!(tools.len(), 1);
		assert_eq!(tools[0].name, "ng_select_node");

		let res = client
			.call_tool("ng_select_node", json::object! { name: "Head" })
			.expect("call_tool");
		assert_eq!(res["content"][0]["text"].as_str(), Some("Selected Head"));

		let sent = String::from_utf8(client.writer).unwrap();
		let sent_lines: Vec<&str> = sent.lines().collect();
		assert_eq!(sent_lines.len(), 4);
		assert_eq!(json::parse(sent_lines[0]).unwrap()["method"].as_str(), Some("initialize"));
		assert_eq!(
			json::parse(sent_lines[1]).unwrap()["method"].as_str(),
			Some("notifications/initialized")
		);
		assert_eq!(json::parse(sent_lines[2]).unwrap()["method"].as_str(), Some("tools/list"));
		assert_eq!(json::parse(sent_lines[3]).unwrap()["method"].as_str(), Some("tools/call"));
	}

	#[test]
	fn mcp_client_propagates_tool_errors_and_rpc_errors() {
		let server_lines = [
			r#"{"jsonrpc":"2.0","id":1,"result":{"isError":true,"content":[{"type":"text","text":"Node not found"}]}}"#,
			r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#,
		]
		.join("\n")
			+ "\n";
		let mut client = McpClient::new(Cursor::new(server_lines.into_bytes()), Vec::<u8>::new());
		let tool_err = client.call_tool("missing_tool", JsonValue::new_object()).unwrap_err();
		assert!(tool_err.contains("Node not found"));

		let rpc_err = client.ping().unwrap_err();
		assert!(rpc_err.contains("-32601"));
	}
}
