//! NDJSON codec for message serialization
//!
//! This module provides shared encoding/decoding logic for the line-delimited
//! JSON protocol used for socket communication.

use super::SocketError;
use crate::protocol::ServerMessage;

/// Encode a server message into NDJSON format
pub fn encode_server_message(message: &ServerMessage) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(message)?;
    json.push('\n');
    Ok(json)
}

/// Decode a line of JSON into a ServerMessage
pub fn decode_server_message(line: &str) -> Result<ServerMessage, SocketError> {
    let message: ServerMessage = serde_json::from_str(line.trim())?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::State;

    #[test]
    fn test_server_message_result_roundtrip() {
        use uuid::Uuid;

        let message = ServerMessage::new_result(
            Uuid::new_v4(),
            "test text".to_string(),
            1.5,
            "model".to_string(),
        );
        let encoded = encode_server_message(&message).unwrap();

        let decoded = decode_server_message(encoded.trim()).unwrap();
        assert!(matches!(decoded, ServerMessage::Result { .. }));
    }

    #[test]
    fn test_server_message_status_event_roundtrip() {
        let message = ServerMessage::new_status_event(State::Idle, None, true, 1000);
        let encoded = encode_server_message(&message).unwrap();

        let decoded = decode_server_message(encoded.trim()).unwrap();
        assert!(matches!(decoded, ServerMessage::StatusEvent { .. }));
    }
}
