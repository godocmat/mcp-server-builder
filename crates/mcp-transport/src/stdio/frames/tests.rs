use std::time::Duration;

use rmcp::model::{
    CallToolResult, ContentBlock, EmptyResult, ProgressNotification, ProgressNotificationParam,
    ServerResult,
};
use serde_json::{Value, json};
use tokio::io::{DuplexStream, ReadHalf};

use super::*;

const WAIT: Duration = Duration::from_secs(2);

fn request(id: i64, progress: Option<&str>) -> Vec<u8> {
    let mut meta = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{}
    });
    if let Some(progress) = progress {
        meta["progressToken"] = json!(progress);
    }
    let mut frame = serde_json::to_vec(&json!({
        "jsonrpc":"2.0","id":id,"method":"tools/list","params":{"_meta":meta}
    }))
    .unwrap();
    frame.push(b'\n');
    frame
}

async fn response(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Value {
    let mut bytes = Vec::new();
    let count = tokio::time::timeout(WAIT, reader.read_until(b'\n', &mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(count <= MAX_FRAME_BYTES);
    assert_eq!(bytes.last(), Some(&b'\n'));
    serde_json::from_slice(&bytes).unwrap()
}

async fn receive(transport: &mut StdioTransport<DuplexStream>) -> ClientJsonRpcMessage {
    tokio::time::timeout(WAIT, transport.receive())
        .await
        .unwrap()
        .unwrap()
}

fn completed(id: RequestId) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::response(ServerResult::EmptyResult(EmptyResult {}), id)
}

#[tokio::test]
async fn writer_replaces_oversized_response_and_preserves_original_id() {
    let (mut client_input, server_input) = tokio::io::duplex(1024);
    let (server_output, client_output) = tokio::io::duplex(128);
    let (mut transport, writer) =
        StdioTransport::new(server_input, server_output, CancellationToken::new());
    let mut client_output = BufReader::new(tokio::io::split(client_output).0);

    client_input.write_all(&request(31, None)).await.unwrap();
    let (_, id) = receive(&mut transport).await.into_request().unwrap();
    let output = CallToolResult::success(vec![ContentBlock::text("x".repeat(MAX_FRAME_BYTES))]);
    let terminal = tokio::spawn(transport.send(ServerJsonRpcMessage::response(
        ServerResult::CallToolResult(output),
        id,
    )));
    let rejected = response(&mut client_output).await;
    assert_eq!(rejected["id"], 31);
    assert_eq!(rejected["error"]["code"], -32603);
    assert_eq!(
        rejected["error"]["message"],
        "MCP response exceeds its byte limit."
    );
    terminal.await.unwrap().unwrap();

    client_input.write_all(&request(31, None)).await.unwrap();
    let (_, id) = receive(&mut transport).await.into_request().unwrap();
    let next = tokio::spawn(transport.send(completed(id)));
    assert_eq!(response(&mut client_output).await["id"], 31);
    next.await.unwrap().unwrap();
    transport.close().await.unwrap();
    writer.await.unwrap();
}

#[tokio::test]
async fn partial_rejection_cannot_interleave_and_cancelled_generation_stays_silent() {
    let (mut client_input, server_input) = tokio::io::duplex(1024);
    let (server_output, client_output) = tokio::io::duplex(1);
    let (mut transport, writer) =
        StdioTransport::new(server_input, server_output, CancellationToken::new());
    let mut client_output = BufReader::new(tokio::io::split(client_output).0);
    client_input.write_all(b"{invalid\n").await.unwrap();
    client_input
        .write_all(&request(31, Some("shared")))
        .await
        .unwrap();
    let (incoming, old_id) = receive(&mut transport).await.into_request().unwrap();
    assert_eq!(client_output.fill_buf().await.unwrap(), b"{");

    let progress = ProgressNotification::new(ProgressNotificationParam::new(
        incoming.get_meta().get_progress_token().unwrap(),
        1.0,
    ));
    let old_progress = tokio::spawn(transport.send(ServerJsonRpcMessage::notification(
        ServerNotification::ProgressNotification(progress),
    )));
    let old_terminal = tokio::spawn(transport.send(completed(old_id.clone())));
    client_input.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":31}}\n")
        .await.unwrap();
    let cancellation = receive(&mut transport).await.into_notification().unwrap();
    let ClientNotification::CancelledNotification(cancellation) = cancellation else {
        panic!("expected cancellation");
    };
    assert_eq!(cancellation.params.request_id, Some(old_id.clone()));
    client_input
        .write_all(&request(31, Some("shared")))
        .await
        .unwrap();
    let (_, new_id) = receive(&mut transport).await.into_request().unwrap();
    assert_ne!(new_id, old_id);
    let next = tokio::spawn(transport.send(completed(new_id)));

    let rejected = response(&mut client_output).await;
    assert_eq!(rejected["error"]["code"], -32700);
    assert!(rejected.get("id").is_none());
    let completed = response(&mut client_output).await;
    assert_eq!(completed["id"], 31);
    assert_eq!(completed["result"], json!({}));
    old_progress.await.unwrap().unwrap();
    old_terminal.await.unwrap().unwrap();
    next.await.unwrap().unwrap();
    transport.close().await.unwrap();
    writer.await.unwrap();
    assert!(client_output.fill_buf().await.unwrap().is_empty());
}

#[tokio::test]
async fn active_request_limit_leaves_cancellation_available() {
    let wire = Wire::new(CancellationToken::new());
    let mut lifecycle = Lifecycle::Undecided;
    for id in 0..MAX_IN_FLIGHT as i64 {
        assert!(
            decode(&request(id, None), &wire, &mut lifecycle)
                .unwrap()
                .is_some()
        );
    }
    let full = decode(&request(100, None), &wire, &mut lifecycle).unwrap_err();
    assert_eq!(full.0.code, ErrorCode::INTERNAL_ERROR);
    assert_eq!(full.1, Some(NumberOrString::Number(100)));
    let cancellation =
        br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":0}}"#;
    assert!(
        decode(cancellation, &wire, &mut lifecycle)
            .unwrap()
            .is_some()
    );
    assert!(
        decode(&request(100, None), &wire, &mut lifecycle)
            .unwrap()
            .is_some()
    );
}
