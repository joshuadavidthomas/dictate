//! NDJSON codec for message serialization
//!
//! This module provides shared encoding/decoding logic for the line-delimited
//! JSON protocol used for socket communication.

use crate::protocol::{ClientMessage, ServerMessage};
use crate::socket::SocketError;

/// Encode a client message into NDJSON format (JSON + newline)
pub fn encode_client_message(message: &ClientMessage) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(message)?;
    json.push('\n');
    Ok(json)
}

/// Encode a server message into NDJSON format
pub fn encode_server_message(message: &ServerMessage) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(message)?;
    json.push('\n');
    Ok(json)
}

/// Decode a line of JSON into a ClientMessage
pub fn decode_client_message(line: &str) -> Result<ClientMessage, SocketError> {
    let message: ClientMessage = serde_json::from_str(line.trim())?;
    Ok(message)
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
    fn test_encode_client_message() {
        let message = ClientMessage::new_status();
        let encoded = encode_client_message(&message).unwrap();
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("\"type\":\"status\""));
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let message = ClientMessage::new_transcribe(30, 2, 16000);
        let encoded = encode_client_message(&message).unwrap();

        // Simulate receiving the message (remove newline)
        let line = encoded.trim_end();
        let decoded: ClientMessage = serde_json::from_str(line).unwrap();

        match decoded {
            ClientMessage::Transcribe { max_duration, silence_duration, sample_rate, .. } => {
                assert_eq!(max_duration, 30);
                assert_eq!(silence_duration, 2);
                assert_eq!(sample_rate, 16000);
            }
            _ => panic!("Wrong message type"),
        }
    }

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
