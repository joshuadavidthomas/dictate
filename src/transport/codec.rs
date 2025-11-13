//! NDJSON codec for message serialization
//!
//! This module provides shared encoding/decoding logic for the line-delimited
//! JSON protocol used for socket communication.

use crate::protocol::{Request, Response as ProtocolResponse, Event};
use crate::socket::{Response, SocketError};
use serde::Serialize;

/// Encode a request into NDJSON format (JSON + newline)
pub fn encode_request(request: &Request) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(request)?;
    json.push('\n');
    Ok(json)
}

/// Encode any serializable message into NDJSON format
pub fn encode_message<T: Serialize>(message: &T) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(message)?;
    json.push('\n');
    Ok(json)
}

/// Encode a response into NDJSON format for sending to clients
pub fn encode_response(response: &Response) -> Result<String, SocketError> {
    let mut json = serde_json::to_string(response)?;
    json.push('\n');
    Ok(json)
}

/// Encode an event into NDJSON format wrapped in a Response
pub fn encode_event(event: &Event) -> Result<String, SocketError> {
    let response = Response::from_event(event.clone());
    encode_response(&response)
}

/// Decode a line of JSON into a Response
pub fn decode_response(line: &str) -> Result<Response, SocketError> {
    let response: Response = serde_json::from_str(line.trim())?;
    Ok(response)
}

/// Decode a line of JSON into a protocol Response
pub fn decode_protocol_response(line: &str) -> Result<ProtocolResponse, SocketError> {
    let response: ProtocolResponse = serde_json::from_str(line.trim())?;
    Ok(response)
}

/// Decode a line of JSON into a Request
pub fn decode_request(line: &str) -> Result<Request, SocketError> {
    let request: Request = serde_json::from_str(line.trim())?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

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
}
