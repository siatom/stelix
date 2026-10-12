//! 将四种模型协议转换为工具编排使用的统一消息。
use crate::types::{AiConfig, AiModel, AiProtocol, AiProvider, AiUsage};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

fn endpoint(provider: &AiProvider, model: &str, stream: bool) -> Result<reqwest::Url, String> {
    let mut url =
        reqwest::Url::parse(provider.base_url.trim_end_matches('/')).map_err(|_| "API 地址无效")?;
    let suffix = match provider.protocol {
        AiProtocol::OpenaiChat => "chat/completions".to_string(),
        AiProtocol::OpenaiResponses => "responses".to_string(),
        AiProtocol::Anthropic => "messages".to_string(),
        AiProtocol::Gemini => format!(
            "models/{}:{}",
            model.trim_start_matches("models/"),
            if stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            }
        ),
    };
    {
        let mut segments = url.path_segments_mut().map_err(|_| "API 地址无效")?;
        segments.pop_if_empty();
        for segment in suffix.split('/') {
            segments.push(segment);
        }
    }
    if stream && provider.protocol == AiProtocol::Gemini {
        url.query_pairs_mut().append_pair("alt", "sse");
    }
    Ok(url)
}

/// 按协议写入鉴权 header（本地服务无密钥时不设置）。
fn apply_auth(req: reqwest::RequestBuilder, provider: &AiProvider) -> reqwest::RequestBuilder {
    match provider.protocol {
        AiProtocol::Anthropic => {
            let req = req.header("anthropic-version", "2023-06-01");
            if provider.api_key.trim().is_empty() {
                req
            } else {
                req.header("x-api-key", provider.api_key.trim())
            }
        }
        AiProtocol::Gemini => {
            if provider.api_key.trim().is_empty() {
                req
            } else {
                req.header("x-goog-api-key", provider.api_key.trim())
            }
        }
        _ => {
            if provider.api_key.trim().is_empty() {
                req
            } else {
                req.bearer_auth(provider.api_key.trim())
            }
        }
    }
}

fn build_body(
    protocol: AiProtocol,
    model: &str,
    messages: &[Value],
    tools: Option<&Value>,
    stream: bool,
) -> Value {
    let system = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| text(&m["content"]))
        .collect::<Vec<_>>()
        .join("\n");
    let functions: Vec<Value> = tools
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| t["function"].clone())
        .collect();
    match protocol {
        AiProtocol::OpenaiChat => {
            let clean: Vec<Value> = messages
                .iter()
                .map(|m| {
                    let mut m = m.clone();
                    if let Some(object) = m.as_object_mut() {
                        object.retain(|k, _| !k.starts_with('_'));
                    }
                    m
                })
                .collect();
            let mut body = json!({"model":model,"messages":clean,"stream":stream});
            if stream {
                body["stream_options"] = json!({"include_usage":true});
            }
            if !functions.is_empty() {
                body["tools"] = tools.unwrap().clone();
                body["tool_choice"] = json!("auto");
            }
            body
        }
        AiProtocol::OpenaiResponses => {
            let mut input = Vec::new();
            for m in messages.iter().filter(|m| m["role"] != "system") {
                if let Some(items) = m["_responseOutput"].as_array() {
                    input.extend(items.iter().cloned());
                } else if m["role"] == "tool" {
                    input.push(json!({"type":"function_call_output","call_id":m["tool_call_id"],"output":m["content"]}));
                } else {
                    input.push(json!({"role":m["role"],"content":m["content"]}));
                }
            }
            let mut body = json!({"model":model,"instructions":system,"input":input,"store":false,
                "stream":stream,"include":["reasoning.encrypted_content"]});
            if !functions.is_empty() {
                body["tools"] = json!(functions.iter().map(|f| json!({"type":"function",
                    "name":f["name"],"description":f["description"],"parameters":f["parameters"],"strict":false})).collect::<Vec<_>>());
            }
            body
        }
        AiProtocol::Anthropic => {
            let mut native: Vec<Value> = Vec::new();
            for m in messages.iter().filter(|m| m["role"] != "system") {
                let (role, blocks) = if m["role"] == "tool" {
                    (
                        "user",
                        json!([{"type":"tool_result","tool_use_id":m["tool_call_id"],"content":m["content"]}]),
                    )
                } else if m["_anthropicContent"].is_array() {
                    ("assistant", m["_anthropicContent"].clone())
                } else {
                    (
                        text(&m["role"]),
                        json!([{"type":"text","text":m["content"]}]),
                    )
                };
                // 并行工具的结果必须合并到紧接 assistant 的同一条 user 消息。
                if let Some(last) = native.last_mut().filter(|last| last["role"] == role) {
                    last["content"]
                        .as_array_mut()
                        .unwrap()
                        .extend(blocks.as_array().unwrap().iter().cloned());
                } else {
                    native.push(json!({"role":role,"content":blocks}));
                }
            }
            let mut body = json!({"model":model,"system":system,"messages":native,"max_tokens":4096,"stream":stream});
            if !functions.is_empty() {
                body["tools"] = json!(functions
                    .iter()
                    .map(|f| json!({"name":f["name"],
                    "description":f["description"],"input_schema":f["parameters"]}))
                    .collect::<Vec<_>>());
            }
            body
        }
        AiProtocol::Gemini => {
            let mut contents: Vec<Value> = Vec::new();
            let mut calls = std::collections::HashMap::new();
            for m in messages.iter().filter(|m| m["role"] != "system") {
                if let Some(tool_calls) = m["tool_calls"].as_array() {
                    for call in tool_calls {
                        calls.insert(text(&call["id"]).to_string(), call);
                    }
                }
                if m["_geminiContent"].is_object() {
                    contents.push(m["_geminiContent"].clone());
                    continue;
                }
                let (role, parts) = if m["role"] == "tool" {
                    let call = calls.get(text(&m["tool_call_id"]));
                    let mut response = json!({"name":call.map(|c| text(&c["function"]["name"])).unwrap_or(""),
                        "response":{"result":m["content"]}});
                    if let Some(id) = call.and_then(|c| c.get("_geminiId")) {
                        response["id"] = id.clone();
                    }
                    ("user", json!([{"functionResponse":response}]))
                } else {
                    (
                        if m["role"] == "assistant" {
                            "model"
                        } else {
                            "user"
                        },
                        json!([{"text":m["content"]}]),
                    )
                };
                if let Some(last) = contents.last_mut().filter(|last| last["role"] == role) {
                    last["parts"]
                        .as_array_mut()
                        .unwrap()
                        .extend(parts.as_array().unwrap().iter().cloned());
                } else {
                    contents.push(json!({"role":role,"parts":parts}));
                }
            }
            let mut body =
                json!({"contents":contents,"systemInstruction":{"parts":[{"text":system}]}});
            if !functions.is_empty() {
                body["tools"] = json!([{"functionDeclarations": functions}]);
            }
            body
        }
    }
}

fn normalize(protocol: AiProtocol, data: Value) -> Result<Value, String> {
    if protocol == AiProtocol::OpenaiChat {
        return data
            .pointer("/choices/0/message")
            .cloned()
            .ok_or("模型响应缺少消息".into());
    }
    let mut content = Vec::new();
    let mut calls = Vec::new();
    let mut message = json!({"role":"assistant"});
    let blocks = match protocol {
        AiProtocol::OpenaiResponses => {
            if data["status"] == "failed" || data["status"] == "incomplete" {
                return Err("模型响应失败或达到输出上限".into());
            }
            message["_responseOutput"] = data["output"].clone();
            data["output"].as_array()
        }
        AiProtocol::Anthropic => {
            if data["stop_reason"] == "max_tokens" {
                return Err("模型响应达到输出上限".into());
            }
            message["_anthropicContent"] = data["content"].clone();
            data["content"].as_array()
        }
        AiProtocol::Gemini => {
            let candidate = &data["candidates"][0];
            if let Some(reason) = candidate["finishReason"].as_str() {
                if reason != "STOP" {
                    return Err(format!("模型响应未完成：{reason}"));
                }
            }
            message["_geminiContent"] = candidate["content"].clone();
            candidate["content"]["parts"].as_array()
        }
        AiProtocol::OpenaiChat => unreachable!(),
    }
    .ok_or("模型响应缺少有效内容（可能被模型拒绝）")?;
    for (i, block) in blocks.iter().enumerate() {
        match protocol {
            AiProtocol::OpenaiResponses => match text(&block["type"]) {
                "message" => {
                    for part in block["content"].as_array().into_iter().flatten() {
                        if part["type"] == "output_text" {
                            content.push(text(&part["text"]).to_string());
                        }
                    }
                }
                "function_call" => calls.push(json!({"id":block["call_id"],"type":"function",
                    "function":{"name":block["name"],"arguments":block["arguments"]}})),
                _ => {}
            },
            AiProtocol::Anthropic => match text(&block["type"]) {
                "text" => content.push(text(&block["text"]).to_string()),
                "tool_use" => calls.push(json!({"id":block["id"],"type":"function",
                    "function":{"name":block["name"],"arguments":block["input"].to_string()}})),
                _ => {}
            },
            AiProtocol::Gemini => {
                if block["thought"] != true {
                    if let Some(s) = block["text"].as_str() {
                        content.push(s.to_string());
                    }
                }
                if let Some(call) = block.get("functionCall") {
                    let mut normalized = json!({"id":format!("gemini_{i}"),"type":"function",
                        "function":{"name":call["name"],"arguments":call["args"].as_object().map(|_| call["args"].to_string()).unwrap_or("{}".into())}});
                    if let Some(id) = call.get("id") {
                        normalized["_geminiId"] = id.clone();
                    }
                    calls.push(normalized);
                }
            }
            AiProtocol::OpenaiChat => unreachable!(),
        }
    }
    if content.is_empty() && calls.is_empty() {
        return Err("模型没有返回文本或工具调用".into());
    }
    message["content"] = json!(content.join("\n"));
    message["tool_calls"] = json!(calls);
    Ok(message)
}

pub(super) async fn request(
    cfg: &AiConfig,
    messages: &[Value],
    tools: Option<&Value>,
) -> Result<Value, String> {
    let (provider, model) = cfg.active()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "初始化 HTTP 客户端失败")?;
    let body = build_body(provider.protocol, &model.id, messages, tools, false);
    let req = apply_auth(client.post(endpoint(provider, &model.id, false)?), provider).json(&body);
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求模型失败：{}", e.without_url()))?;
    let status = resp.status();
    if !status.is_success() {
        // 不回显服务端原始响应，避免网关把密钥或请求内容写入错误提示。
        return Err(format!(
            "模型接口返回 HTTP {status}，请检查地址、协议、密钥和模型 ID"
        ));
    }
    let data = resp
        .json::<Value>()
        .await
        .map_err(|_| "模型接口没有返回有效 JSON")?;
    normalize(provider.protocol, data)
}

// ================================================================ 流式（SSE）

/// SSE 帧：`event` 类型 + 拼接后的 `data`。
struct SseFrame {
    event: Option<String>,
    data: String,
}

/// 逐块喂入字节，切出完整 SSE 帧；处理 `event:` / 多行 `data:` / 注释行。
#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8], frames: &mut Vec<SseFrame>) {
        self.buffer.extend_from_slice(chunk);
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            self.line(&line, frames);
        }
    }

    /// 流结束时把缓冲里未以空行结尾的最后一帧吐出。
    fn flush(&mut self, frames: &mut Vec<SseFrame>) {
        self.emit(frames);
    }

    fn line(&mut self, line: &str, frames: &mut Vec<SseFrame>) {
        if line.is_empty() {
            self.emit(frames);
            self.event = None;
        } else if let Some(rest) = line.strip_prefix("event:") {
            self.event = Some(rest.trim_start().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            self.data
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
        // 其余（注释 `:`、id/retry 等）忽略。
    }

    fn emit(&mut self, frames: &mut Vec<SseFrame>) {
        if self.data.is_empty() {
            return;
        }
        frames.push(SseFrame {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        });
    }
}

/// 从响应里取 token 用量（各协议字段不同）。
fn usage_from(protocol: AiProtocol, value: &Value) -> Option<AiUsage> {
    let (input, output) = match protocol {
        AiProtocol::OpenaiChat => (
            value.pointer("/usage/prompt_tokens"),
            value.pointer("/usage/completion_tokens"),
        ),
        AiProtocol::OpenaiResponses | AiProtocol::Anthropic => (
            value.pointer("/usage/input_tokens"),
            value.pointer("/usage/output_tokens"),
        ),
        AiProtocol::Gemini => (
            value.pointer("/usageMetadata/promptTokenCount"),
            value.pointer("/usageMetadata/candidatesTokenCount"),
        ),
    };
    let input = input.and_then(Value::as_u64);
    let output = output.and_then(Value::as_u64);
    match (input, output) {
        (None, None) => None,
        (i, o) => Some(AiUsage {
            input_tokens: i.unwrap_or(0),
            output_tokens: o.unwrap_or(0),
        }),
    }
}

/// 识别 SSE 错误帧（不回显服务端原文，避免泄漏密钥/请求内容）。
fn frame_error(value: &Value) -> Option<String> {
    if value.get("error").is_some_and(|e| !e.is_null())
        || value["type"] == "error"
        || value["type"] == "response.failed"
    {
        return Some("模型接口返回错误，请检查地址、协议、密钥和模型 ID".into());
    }
    None
}

fn consume(
    frames: &mut Vec<SseFrame>,
    acc: &mut Accumulator,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<(), String> {
    for frame in frames.drain(..) {
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(err) = frame_error(&value) {
            return Err(err);
        }
        if let Some(delta) = acc.push(frame.event.as_deref(), &value)? {
            if !delta.is_empty() {
                on_delta(&delta);
            }
        }
    }
    Ok(())
}

/// 把流式 delta 累积成 `normalize()` 可消费的完整响应 JSON。
enum Accumulator {
    OpenaiChat {
        content: String,
        tools: Vec<Value>,
        usage: Option<AiUsage>,
    },
    OpenaiResponses {
        items: Vec<Value>,
        output: Option<Vec<Value>>,
        usage: Option<AiUsage>,
    },
    Anthropic {
        blocks: Vec<Value>,
        args: Vec<String>,
        input_tokens: u64,
        output_tokens: u64,
        stop_reason: Option<String>,
    },
    Gemini {
        parts: Vec<Value>,
        finish: Option<String>,
        usage: Option<AiUsage>,
    },
}

impl Accumulator {
    fn new(protocol: AiProtocol) -> Self {
        match protocol {
            AiProtocol::OpenaiChat => Accumulator::OpenaiChat {
                content: String::new(),
                tools: Vec::new(),
                usage: None,
            },
            AiProtocol::OpenaiResponses => Accumulator::OpenaiResponses {
                items: Vec::new(),
                output: None,
                usage: None,
            },
            AiProtocol::Anthropic => Accumulator::Anthropic {
                blocks: Vec::new(),
                args: Vec::new(),
                input_tokens: 0,
                output_tokens: 0,
                stop_reason: None,
            },
            AiProtocol::Gemini => Accumulator::Gemini {
                parts: Vec::new(),
                finish: None,
                usage: None,
            },
        }
    }

    fn push(&mut self, _event: Option<&str>, value: &Value) -> Result<Option<String>, String> {
        match self {
            Accumulator::OpenaiChat {
                content,
                tools,
                usage,
            } => {
                let mut delta_text = String::new();
                for choice in value["choices"].as_array().into_iter().flatten() {
                    let delta = &choice["delta"];
                    if let Some(t) = delta["content"].as_str() {
                        content.push_str(t);
                        delta_text.push_str(t);
                    }
                    for call in delta["tool_calls"].as_array().into_iter().flatten() {
                        let index = call["index"].as_u64().unwrap_or(0) as usize;
                        while tools.len() <= index {
                            tools.push(json!({"id":"","type":"function","function":{"name":"","arguments":""}}));
                        }
                        let entry = &mut tools[index];
                        if let Some(id) = call["id"].as_str() {
                            if !id.is_empty() {
                                entry["id"] = json!(id);
                            }
                        }
                        if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                            entry["function"]["name"] = json!(format!(
                                "{}{}",
                                entry["function"]["name"].as_str().unwrap_or(""),
                                name
                            ));
                        }
                        if let Some(args) =
                            call.pointer("/function/arguments").and_then(Value::as_str)
                        {
                            entry["function"]["arguments"] = json!(format!(
                                "{}{}",
                                entry["function"]["arguments"].as_str().unwrap_or(""),
                                args
                            ));
                        }
                    }
                }
                if let Some(u) = usage_from(AiProtocol::OpenaiChat, value) {
                    *usage = Some(u);
                }
                Ok(if delta_text.is_empty() {
                    None
                } else {
                    Some(delta_text)
                })
            }
            Accumulator::OpenaiResponses {
                items,
                output,
                usage,
            } => {
                let mut delta_text = None;
                match value["type"].as_str().unwrap_or("") {
                    "response.output_text.delta" => {
                        if let Some(d) = value["delta"].as_str() {
                            delta_text = Some(d.to_string());
                        }
                    }
                    "response.output_item.done" => {
                        if let Some(item) = value.get("item") {
                            items.push(item.clone());
                        }
                    }
                    "response.completed" => {
                        let response = &value["response"];
                        if let Some(out) = response["output"].as_array() {
                            *output = Some(out.clone());
                        }
                        if let Some(u) = usage_from(AiProtocol::OpenaiResponses, response) {
                            *usage = Some(u);
                        }
                    }
                    _ => {}
                }
                Ok(delta_text)
            }
            Accumulator::Anthropic {
                blocks,
                args,
                input_tokens,
                output_tokens,
                stop_reason,
            } => {
                let mut delta_text = None;
                match value["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        *input_tokens = value
                            .pointer("/message/usage/input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                    }
                    "content_block_start" => {
                        let index = value["index"].as_u64().unwrap_or(0) as usize;
                        while blocks.len() <= index {
                            blocks.push(Value::Null);
                        }
                        while args.len() <= index {
                            args.push(String::new());
                        }
                        blocks[index] = value["content_block"].clone();
                    }
                    "content_block_delta" => {
                        let index = value["index"].as_u64().unwrap_or(0) as usize;
                        let delta = &value["delta"];
                        match delta["type"].as_str().unwrap_or("") {
                            "text_delta" => {
                                let t = delta["text"].as_str().unwrap_or("");
                                if !t.is_empty() {
                                    while blocks.len() <= index {
                                        blocks.push(Value::Null);
                                    }
                                    if !blocks[index].is_object() {
                                        blocks[index] = json!({"type":"text","text":""});
                                    }
                                    if blocks[index]["type"] != "tool_use" {
                                        blocks[index]["text"] = json!(format!(
                                            "{}{}",
                                            blocks[index]["text"].as_str().unwrap_or(""),
                                            t
                                        ));
                                    }
                                    delta_text = Some(t.to_string());
                                }
                            }
                            "input_json_delta" => {
                                while args.len() <= index {
                                    args.push(String::new());
                                }
                                args[index].push_str(delta["partial_json"].as_str().unwrap_or(""));
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let index = value["index"].as_u64().unwrap_or(0) as usize;
                        if let Some(raw) = args.get(index) {
                            if !raw.is_empty() {
                                if let Some(block) = blocks.get_mut(index) {
                                    block["input"] =
                                        serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
                                }
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(r) = value.pointer("/delta/stop_reason").and_then(Value::as_str)
                        {
                            *stop_reason = Some(r.to_string());
                        }
                        if let Some(o) = value
                            .pointer("/usage/output_tokens")
                            .and_then(Value::as_u64)
                        {
                            *output_tokens = o;
                        }
                    }
                    _ => {}
                }
                Ok(delta_text)
            }
            Accumulator::Gemini {
                parts,
                finish,
                usage,
            } => {
                let mut delta_text = String::new();
                let mut extras: Vec<Value> = Vec::new();
                for candidate in value["candidates"].as_array().into_iter().flatten() {
                    if let Some(reason) = candidate["finishReason"].as_str() {
                        *finish = Some(reason.to_string());
                    }
                    for part in candidate["content"]["parts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let thought = part["thought"] == true;
                        if let Some(t) = part["text"].as_str() {
                            if !thought {
                                delta_text.push_str(t);
                                let pure_text =
                                    part.as_object().map(|o| o.len() == 1).unwrap_or(false);
                                let last_is_text = parts
                                    .last()
                                    .map(|p| p["thought"] != true && p.get("text").is_some())
                                    .unwrap_or(false);
                                if pure_text && last_is_text {
                                    let last = parts.last_mut().unwrap();
                                    last["text"] = json!(format!(
                                        "{}{}",
                                        last["text"].as_str().unwrap_or(""),
                                        t
                                    ));
                                } else {
                                    parts.push(json!({"text": t}));
                                }
                                continue;
                            }
                        }
                        extras.push(part.clone());
                    }
                }
                parts.extend(extras);
                if let Some(u) = usage_from(AiProtocol::Gemini, value) {
                    *usage = Some(u);
                }
                Ok(if delta_text.is_empty() {
                    None
                } else {
                    Some(delta_text)
                })
            }
        }
    }

    fn usage(&self) -> Option<AiUsage> {
        match self {
            Accumulator::OpenaiChat { usage, .. }
            | Accumulator::OpenaiResponses { usage, .. }
            | Accumulator::Gemini { usage, .. } => *usage,
            Accumulator::Anthropic {
                input_tokens,
                output_tokens,
                ..
            } => {
                if *input_tokens == 0 && *output_tokens == 0 {
                    None
                } else {
                    Some(AiUsage {
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                    })
                }
            }
        }
    }

    fn finish(self) -> Result<Value, String> {
        Ok(match self {
            Accumulator::OpenaiChat { content, tools, .. } => {
                let mut message = json!({"role":"assistant","content":content});
                if !tools.is_empty() {
                    message["tool_calls"] = json!(tools);
                }
                json!({"choices":[{"message":message}]})
            }
            Accumulator::OpenaiResponses { items, output, .. } => {
                json!({"output": output.unwrap_or(items)})
            }
            Accumulator::Anthropic {
                blocks,
                stop_reason,
                ..
            } => {
                let content: Vec<Value> = blocks.into_iter().filter(|b| !b.is_null()).collect();
                let mut out = json!({"content": content});
                if let Some(r) = stop_reason {
                    out["stop_reason"] = json!(r);
                }
                out
            }
            Accumulator::Gemini { parts, finish, .. } => {
                let mut candidate = json!({"content":{"role":"model","parts":parts}});
                if let Some(f) = finish {
                    candidate["finishReason"] = json!(f);
                }
                json!({"candidates":[candidate]})
            }
        })
    }
}

/// 流式请求：把 SSE delta 累积成与非流式等价的归一化消息，并逐段回传文本增量。
///
/// - `cancel` 置位后会在下一次轮询（≤250ms）立即停止，丢弃后续增量。
/// - 网关忽略 `stream` 时按一次性 JSON 兜底。
pub(super) async fn stream(
    cfg: &AiConfig,
    messages: &[Value],
    tools: Option<&Value>,
    cancel: &AtomicBool,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<(Value, Option<AiUsage>), String> {
    let (provider, model) = cfg.active()?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "初始化 HTTP 客户端失败")?;
    let body = build_body(provider.protocol, &model.id, messages, tools, true);
    let req = apply_auth(client.post(endpoint(provider, &model.id, true)?), provider).json(&body);
    let mut resp = req
        .send()
        .await
        .map_err(|e| format!("请求模型失败：{}", e.without_url()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "模型接口返回 HTTP {status}，请检查地址、协议、密钥和模型 ID"
        ));
    }
    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("text/event-stream"));
    if !is_sse {
        let data = resp
            .json::<Value>()
            .await
            .map_err(|_| "模型接口没有返回有效 JSON")?;
        let usage = usage_from(provider.protocol, &data);
        return Ok((normalize(provider.protocol, data)?, usage));
    }

    let mut parser = SseParser::default();
    let mut frames: Vec<SseFrame> = Vec::new();
    let mut acc = Accumulator::new(provider.protocol);
    let mut last_data = tokio::time::Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(250), resp.chunk()).await {
            Err(_) => {
                if last_data.elapsed() > Duration::from_secs(30) {
                    return Err("模型响应超时（30 秒无数据）".into());
                }
            }
            Ok(Err(e)) => return Err(format!("读取模型响应失败：{}", e.without_url())),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(bytes))) => {
                last_data = tokio::time::Instant::now();
                parser.push(&bytes, &mut frames);
                consume(&mut frames, &mut acc, on_delta)?;
            }
        }
    }
    parser.flush(&mut frames);
    consume(&mut frames, &mut acc, on_delta)?;
    let usage = acc.usage();
    let message = normalize(provider.protocol, acc.finish()?)?;
    Ok((message, usage))
}

// ================================================================ 模型列表

/// 拉取 Provider 可用的模型列表（按协议 GET `{base}/models`）。
pub(super) async fn list_models(provider: &AiProvider) -> Result<Vec<AiModel>, String> {
    crate::types::validate_provider_url(&provider.base_url)?;
    let mut url =
        reqwest::Url::parse(provider.base_url.trim_end_matches('/')).map_err(|_| "API 地址无效")?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| "API 地址无效")?;
        segments.pop_if_empty();
        segments.push("models");
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "初始化 HTTP 客户端失败")?;
    let resp = apply_auth(client.get(url), provider)
        .send()
        .await
        .map_err(|e| format!("请求模型列表失败：{}", e.without_url()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "获取模型列表返回 HTTP {status}，请检查地址、协议和密钥"
        ));
    }
    let data = resp
        .json::<Value>()
        .await
        .map_err(|_| "模型列表没有返回有效 JSON")?;
    let mut models = Vec::new();
    match provider.protocol {
        AiProtocol::OpenaiChat | AiProtocol::OpenaiResponses | AiProtocol::Anthropic => {
            for item in data["data"].as_array().into_iter().flatten() {
                let id = item["id"].as_str().unwrap_or("");
                let name = item["display_name"].as_str().unwrap_or(id);
                if let Some(m) = normalize_model(id, name) {
                    models.push(m);
                }
            }
        }
        AiProtocol::Gemini => {
            for item in data["models"].as_array().into_iter().flatten() {
                let raw = item["name"].as_str().unwrap_or("");
                let id = raw.trim_start_matches("models/");
                let name = item["displayName"].as_str().unwrap_or(id);
                if let Some(m) = normalize_model(id, name) {
                    models.push(m);
                }
            }
        }
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    models.truncate(300);
    if models.is_empty() {
        return Err("未获取到模型，请确认协议、地址与密钥".into());
    }
    Ok(models)
}

fn normalize_model(id: &str, name: &str) -> Option<AiModel> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let name = name.trim();
    Some(AiModel {
        id: id.to_string(),
        name: if name.is_empty() {
            id.to_string()
        } else {
            name.to_string()
        },
        vendor: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 实际发 HTTP 请求，验证地址、鉴权和所选模型，不调用外部服务。
    #[tokio::test]
    async fn all_protocols_send_expected_http_requests() {
        use crate::types::{AiModel, AiProvider};
        use std::io::{Read, Write};
        use std::net::TcpListener;
        for (protocol, suffix, header, response) in [
            (
                AiProtocol::OpenaiChat,
                "chat/completions",
                "authorization: Bearer test-key",
                json!({"choices":[{"message":{"role":"assistant","content":"OK"}}]}),
            ),
            (
                AiProtocol::OpenaiResponses,
                "responses",
                "authorization: Bearer test-key",
                json!({"output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}]}),
            ),
            (
                AiProtocol::Anthropic,
                "messages",
                "x-api-key: test-key",
                json!({"content":[{"type":"text","text":"OK"}]}),
            ),
            (
                AiProtocol::Gemini,
                "models/test-model:generateContent",
                "x-goog-api-key: test-key",
                json!({"candidates":[{"content":{"role":"model","parts":[{"text":"OK"}]},"finishReason":"STOP"}]}),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let (headers, body) = loop {
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|s| s.trim().parse().unwrap())
                            })
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            let body: Value =
                                serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                            break (headers, body);
                        }
                    }
                };
                let payload = response.to_string();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",payload.len(),payload).unwrap();
                (headers, body)
            });
            let cfg = AiConfig {
                enabled: true,
                active_provider_id: "test".into(),
                active_model_id: "test-model".into(),
                providers: vec![AiProvider {
                    id: "test".into(),
                    name: "Test".into(),
                    vendor: "custom".into(),
                    protocol,
                    base_url: format!("http://{address}/v1/"),
                    api_key: "test-key".into(),
                    models: vec![AiModel {
                        id: "test-model".into(),
                        name: "Test model".into(),
                        vendor: String::new(),
                    }],
                }],
            };
            cfg.validate().unwrap();
            let result = request(
                &cfg,
                &[json!({"role":"user","content":"Hello"})],
                Some(&tools()),
            )
            .await
            .unwrap();
            assert_eq!(result["content"], "OK");
            let (headers, body) = server.join().unwrap();
            assert!(
                headers.starts_with(&format!("POST /v1/{suffix} HTTP/1.1")),
                "{headers}"
            );
            assert!(headers.contains(header));
            if protocol == AiProtocol::Gemini {
                assert_eq!(body["contents"][0]["parts"][0]["text"], "Hello");
            } else {
                assert_eq!(body["model"], "test-model");
            }
            if protocol == AiProtocol::Anthropic {
                assert!(headers.contains("anthropic-version: 2023-06-01"));
            }
        }
    }

    fn tools() -> Value {
        json!([{"type":"function","function":{"name":"list_dir","description":"list","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}])
    }

    #[test]
    fn native_tool_roundtrips_preserve_signatures_and_results() {
        let cases = [
            (
                AiProtocol::OpenaiResponses,
                json!({"output":[{"type":"reasoning","encrypted_content":"opaque"},{"type":"function_call","call_id":"c1","name":"list_dir","arguments":"{\"path\":\"D:\\\"}"}]}),
            ),
            (
                AiProtocol::Anthropic,
                json!({"content":[{"type":"tool_use","id":"c1","name":"list_dir","input":{"path":"D:"}}]}),
            ),
            (
                AiProtocol::Gemini,
                json!({"candidates":[{"content":{"role":"model","parts":[{"thoughtSignature":"signature","functionCall":{"name":"list_dir","args":{"path":"D:"}}}]},"finishReason":"STOP"}]}),
            ),
        ];
        for (protocol, data) in cases {
            let assistant = normalize(protocol, data).unwrap();
            let id = assistant["tool_calls"][0]["id"].clone();
            let messages = vec![
                json!({"role":"system","content":"system"}),
                json!({"role":"user","content":"list"}),
                assistant,
                json!({"role":"tool","tool_call_id":id,"content":"done"}),
            ];
            let body = build_body(protocol, "model", &messages, Some(&tools()), false);
            match protocol {
                AiProtocol::OpenaiResponses => {
                    assert_eq!(body["input"][1]["encrypted_content"], "opaque");
                    assert_eq!(body["input"][3]["call_id"], "c1");
                }
                AiProtocol::Anthropic => {
                    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "c1");
                    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
                }
                AiProtocol::Gemini => {
                    assert_eq!(
                        body["contents"][1]["parts"][0]["thoughtSignature"],
                        "signature"
                    );
                    assert_eq!(
                        body["contents"][2]["parts"][0]["functionResponse"]["name"],
                        "list_dir"
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn parallel_anthropic_results_are_one_user_turn() {
        let messages = vec![
            json!({"role":"assistant","_anthropicContent":[{"type":"tool_use","id":"a","name":"list_dir","input":{}},{"type":"tool_use","id":"b","name":"list_dir","input":{}}]}),
            json!({"role":"tool","tool_call_id":"a","content":"a"}),
            json!({"role":"tool","tool_call_id":"b","content":"b"}),
        ];
        let body = build_body(
            AiProtocol::Anthropic,
            "model",
            &messages,
            Some(&tools()),
            false,
        );
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn planning_text_and_failures_are_normalized() {
        for (protocol, data) in [
            (
                AiProtocol::OpenaiChat,
                json!({"choices":[{"message":{"role":"assistant","content":"draft"}}]}),
            ),
            (
                AiProtocol::OpenaiResponses,
                json!({"output":[{"type":"message","content":[{"type":"output_text","text":"draft"}]}]}),
            ),
            (
                AiProtocol::Anthropic,
                json!({"content":[{"type":"text","text":"draft"}]}),
            ),
            (
                AiProtocol::Gemini,
                json!({"candidates":[{"content":{"role":"model","parts":[{"text":"draft"}]},"finishReason":"STOP"}]}),
            ),
        ] {
            assert_eq!(normalize(protocol, data).unwrap()["content"], "draft");
        }
        assert!(normalize(
            AiProtocol::Gemini,
            json!({"promptFeedback":{"blockReason":"SAFETY"}})
        )
        .is_err());
        assert!(normalize(
            AiProtocol::OpenaiResponses,
            json!({"status":"incomplete","output":[]})
        )
        .is_err());
    }

    // ---------------------------------------------------------------- 流式

    /// 读一次 HTTP 请求（无 body 时 body 为 Null）。
    fn read_http_request(socket: &mut std::net::TcpStream) -> (String, Value) {
        use std::io::Read;
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        let end = loop {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                break end;
            }
        };
        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() < end + 4 + length {
            let count = socket.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let body = if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap()
        };
        (headers, body)
    }

    /// 假 SSE 服务端：返回 `text/event-stream`，按 (分片, 延迟) 依次写入后关闭。
    fn serve_stream(
        parts: Vec<(String, Duration)>,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let _ = read_http_request(&mut socket);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            socket.flush().unwrap();
            for (part, delay) in parts {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                socket.write_all(part.as_bytes()).unwrap();
                socket.flush().unwrap();
            }
        });
        (address, handle)
    }

    fn sse(frames: &[(&str, &str)]) -> String {
        let mut out = String::new();
        for (event, data) in frames {
            if !event.is_empty() {
                out.push_str(&format!("event: {event}\n"));
            }
            for line in data.split('\n') {
                out.push_str(&format!("data: {line}\n"));
            }
            out.push('\n');
        }
        out
    }

    fn stream_cfg(address: std::net::SocketAddr, protocol: AiProtocol) -> AiConfig {
        AiConfig {
            enabled: true,
            active_provider_id: "test".into(),
            active_model_id: "test-model".into(),
            providers: vec![AiProvider {
                id: "test".into(),
                name: "Test".into(),
                vendor: "custom".into(),
                protocol,
                base_url: format!("http://{address}/v1"),
                api_key: "test-key".into(),
                models: vec![AiModel {
                    id: "test-model".into(),
                    name: "Test model".into(),
                    vendor: String::new(),
                }],
            }],
        }
    }

    #[test]
    fn sse_parser_splits_frames_across_byte_chunks() {
        let raw = sse(&[("", "{\"a\":1}"), ("content_block_delta", "{\"b\":2}")]);
        let mut parser = SseParser::default();
        let mut frames = Vec::new();
        for byte in raw.as_bytes() {
            parser.push(&[*byte], &mut frames);
        }
        parser.flush(&mut frames);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "{\"a\":1}");
        assert_eq!(frames[1].event.as_deref(), Some("content_block_delta"));
        assert_eq!(frames[1].data, "{\"b\":2}");
    }

    #[tokio::test]
    async fn streaming_matches_non_streaming_for_all_protocols() {
        let cases: Vec<(AiProtocol, Vec<(&str, &str)>, Value, &str, u64, u64)> = vec![
            (
                AiProtocol::OpenaiChat,
                vec![
                    ("", "{\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"你\"}}]}"),
                    ("", "{\"choices\":[{\"delta\":{\"content\":\"好\"}}]}"),
                    ("", "{\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}"),
                    ("", "[DONE]"),
                ],
                json!({"choices":[{"message":{"role":"assistant","content":"你好"}}]}),
                "你好",
                3,
                2,
            ),
            (
                AiProtocol::OpenaiResponses,
                vec![
                    ("response.output_text.delta", "{\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}"),
                    ("response.completed", "{\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"OK\"}]}],\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}"),
                    ("", "[DONE]"),
                ],
                json!({"output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}]}),
                "OK",
                4,
                1,
            ),
            (
                AiProtocol::Anthropic,
                vec![
                    ("message_start", "{\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}"),
                    ("content_block_start", "{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}"),
                    ("content_block_delta", "{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"O\"}}"),
                    ("content_block_delta", "{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"K\"}}"),
                    ("content_block_stop", "{\"type\":\"content_block_stop\",\"index\":0}"),
                    ("message_delta", "{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}"),
                    ("message_stop", "{\"type\":\"message_stop\"}"),
                ],
                json!({"content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn"}),
                "OK",
                5,
                2,
            ),
            (
                AiProtocol::Gemini,
                vec![
                    ("", "{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"O\"}]}}]}"),
                    ("", "{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"K\"}]}}],\"usageMetadata\":{\"promptTokenCount\":6,\"candidatesTokenCount\":2}}"),
                    ("", "{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}]}"),
                ],
                json!({"candidates":[{"content":{"role":"model","parts":[{"text":"OK"}]},"finishReason":"STOP"}]}),
                "OK",
                6,
                2,
            ),
        ];
        for (protocol, frames, full, expected, input, output) in cases {
            let (address, server) = serve_stream(vec![(sse(&frames), Duration::ZERO)]);
            let cfg = stream_cfg(address, protocol);
            let cancel = AtomicBool::new(false);
            let mut collected = String::new();
            let (message, usage) = stream(
                &cfg,
                &[json!({"role":"user","content":"hi"})],
                None,
                &cancel,
                &mut |d: &str| collected.push_str(d),
            )
            .await
            .unwrap();
            server.join().unwrap();
            assert_eq!(collected, expected, "{protocol:?} 增量");
            assert_eq!(message["content"], expected, "{protocol:?} 内容");
            assert_eq!(
                message["content"],
                normalize(protocol, full).unwrap()["content"],
                "{protocol:?} 与非流式不一致"
            );
            let usage = usage.expect("应返回用量");
            assert_eq!(
                (usage.input_tokens, usage.output_tokens),
                (input, output),
                "{protocol:?} 用量"
            );
        }
    }

    #[tokio::test]
    async fn non_sse_response_falls_back_to_json() {
        use std::io::Write;
        use std::net::TcpListener;
        let response = json!({"choices":[{"message":{"role":"assistant","content":"plain"}}]});
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let _ = read_http_request(&mut socket);
            let payload = response.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", payload.len(), payload).unwrap();
        });
        let cfg = stream_cfg(address, AiProtocol::OpenaiChat);
        let cancel = AtomicBool::new(false);
        let mut collected = String::new();
        let (message, usage) = stream(
            &cfg,
            &[json!({"role":"user","content":"hi"})],
            None,
            &cancel,
            &mut |d: &str| collected.push_str(d),
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(message["content"], "plain");
        assert!(collected.is_empty());
        assert!(usage.is_none());
    }

    #[tokio::test]
    async fn cancel_stops_streaming_before_later_deltas() {
        use std::sync::Arc;
        let (address, server) = serve_stream(vec![
            (
                "data: {\"choices\":[{\"delta\":{\"content\":\"A\"}}]}\n\n".to_string(),
                Duration::ZERO,
            ),
            (
                "data: {\"choices\":[{\"delta\":{\"content\":\"B\"}}]}\n\n".to_string(),
                Duration::from_millis(700),
            ),
            ("data: [DONE]\n\n".to_string(), Duration::ZERO),
        ]);
        let cfg = stream_cfg(address, AiProtocol::OpenaiChat);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::Relaxed);
        });
        let mut collected = String::new();
        let started = std::time::Instant::now();
        let (message, _) = stream(
            &cfg,
            &[json!({"role":"user","content":"hi"})],
            None,
            &cancel,
            &mut |d: &str| collected.push_str(d),
        )
        .await
        .unwrap();
        trigger.join().unwrap();
        server.join().unwrap();
        assert_eq!(collected, "A");
        assert_eq!(message["content"], "A");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn list_models_reads_each_protocol_shape() {
        use std::io::Write;
        use std::net::TcpListener;
        for (protocol, response, expected) in [
            (
                AiProtocol::OpenaiChat,
                json!({"data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"}]}),
                vec!["gpt-4o", "gpt-4o-mini"],
            ),
            (
                AiProtocol::Anthropic,
                json!({"data":[{"id":"claude-3","display_name":"Claude 3"}]}),
                vec!["claude-3"],
            ),
            (
                AiProtocol::Gemini,
                json!({"models":[{"name":"models/gemini-2.0","displayName":"Gemini 2.0"}]}),
                vec!["gemini-2.0"],
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let (headers, _) = read_http_request(&mut socket);
                let payload = response.to_string();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", payload.len(), payload).unwrap();
                headers
            });
            let cfg = stream_cfg(address, protocol);
            let provider = cfg.providers.into_iter().next().unwrap();
            let models = list_models(&provider).await.unwrap();
            let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
            assert_eq!(ids, expected, "{protocol:?}");
            let headers = server.join().unwrap();
            assert!(headers.starts_with("GET /v1/models HTTP/1.1"), "{headers}");
        }
    }
}
