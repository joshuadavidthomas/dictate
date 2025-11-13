//! NDJSON codec for message serialization
//!
//! This module provides shared encoding/decoding logic for the line-delimited
//! JSON protocol used for socket communication.

use crate::protocol::{Request, Response, Event, Message};
use crate::socket::SocketError;

/// Encode a request into NDJSON format (JSON + newline)
pub fn encode_request(request: &Request) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(request)?;
    json.push('\n');
    Ok(json)
}

/// Encode a response into NDJSON format
pub fn encode_response(response: &Response) -> Result<String, SocketError> {
    let message = Message::Response(response.clone());
    let mut json = serde_json::to_string(&message)?;
    json.push('\n');
    Ok(json)
}

/// Encode an event into NDJSON format
pub fn encode_event(event: &Event) -> Result<String, SocketError> {
    let message = Message::Event(event.clone());
    let mut json = serde_json::to_string(&message)?;
    json.push('\n');
    Ok(json)
}

/// Decode a line of JSON into a Message
pub fn decode_message(line: &str) -> Result<Message, SocketError> {
    let message: Message = serde_json::from_str(line.trim())?;
    Ok(message)
}

/// Encode a message into NDJSON format (used in tests)
#[cfg(test)]
pub fn encode_message(message: &Message) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(message)?;
    json.push('\n');
    Ok(json)
}

/// Decode a line of JSON into a Request (used in tests)
#[cfg(test)]
pub fn decode_request(line: &str) -> Result<Request, SocketError> {
    let request: Request = serde_json::from_str(line.trim())?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::State;

    #[test]
    fn test_encode_request() {
        let request = Request::new_status();
        let encoded = encode_request(&request).unwrap();
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("\"type\":\"status\""));
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let request = Request::new_transcribe(30, 2, 16000);
        let encoded = encode_request(&request).unwrap();

        // Simulate receiving the message (remove newline)
        let line = encoded.trim_end();
        let decoded: Request = serde_json::from_str(line).unwrap();

        match decoded {
            Request::Transcribe { max_duration, silence_duration, sample_rate, .. } => {
                assert_eq!(max_duration, 30);
                assert_eq!(silence_duration, 2);
                assert_eq!(sample_rate, 16000);
            }
            _ => panic!("Wrong request type"),
        }
    }

    #[test]
    fn test_message_response_roundtrip() {
        use uuid::Uuid;

        let response = Response::new_result(
            Uuid::new_v4(),
            "test text".to_string(),
            1.5,
            "model".to_string(),
        );
        let encoded = encode_response(&response).unwrap();

        let decoded = decode_message(encoded.trim()).unwrap();
        assert!(matches!(decoded, Message::Response(_)));
    }

    #[test]
    fn test_message_event_roundtrip() {
        let event = Event::new_status(State::Idle, None, true, 1000);
        let encoded = encode_event(&event).unwrap();

        let decoded = decode_message(encoded.trim()).unwrap();
        assert!(matches!(decoded, Message::Event(_)));
    }
}
