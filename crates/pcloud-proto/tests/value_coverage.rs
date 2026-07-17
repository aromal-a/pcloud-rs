//! Public value-wrapper and response-parser edge coverage.

use pcloud_proto::{
    RedactedProtoString,
    response::{ParseLimits, ResponseParseError, Value, parse_response_frame},
};

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

#[test]
fn redacted_proto_string_conversion_surface_preserves_explicit_access_only() {
    let owned = RedactedProtoString::from("secret".to_owned());
    assert_eq!(owned.as_ref(), "secret");
    assert_eq!(&*owned, "secret");
    assert_eq!(owned.clone().into_string(), "secret");
    let converted: String = RedactedProtoString::from("converted").into();
    assert_eq!(converted, "converted");
    assert_eq!(
        format!("{}", RedactedProtoString::from("hidden")),
        "<redacted>"
    );
}

#[test]
fn value_accessors_and_malformed_frames_cover_every_closed_wire_shape() {
    let values = [
        Value::String("text".to_owned()),
        Value::Number(7),
        Value::Data(8),
        Value::Bool(true),
        Value::Array(vec![Value::Number(1)]),
        Value::Hash(vec![("key".to_owned(), Value::String("value".to_owned()))]),
    ];
    assert_eq!(values[0].as_string(), Some("text"));
    assert_eq!(values[1].as_number(), Some(7));
    assert_eq!(values[2].as_number(), Some(8));
    assert_eq!(values[3].as_bool(), Some(true));
    assert_eq!(values[4].as_array().unwrap().len(), 1);
    assert_eq!(
        values[5].as_hash().unwrap().get_string("key"),
        Some("value")
    );
    assert!(values[0].as_hash().is_none());
    assert!(values[1].as_string().is_none());
    assert!(values[3].as_number().is_none());
    assert!(values[4].as_bool().is_none());
    assert!(values[5].as_array().is_none());

    let defaults = ParseLimits::default();
    for (body, expected) in [
        (vec![220], ResponseParseError::InvalidTag(220)),
        (vec![150], ResponseParseError::InvalidReuseReference(0)),
        (vec![8], ResponseParseError::UnexpectedEof),
        (vec![200, 200], ResponseParseError::TrailingBytes),
        (
            vec![16, 200, 200, 255],
            ResponseParseError::InvalidHashKeyType,
        ),
    ] {
        assert_eq!(
            parse_response_frame(&frame(&body), &defaults).unwrap_err(),
            expected
        );
    }

    let mut limits = defaults.clone();
    limits.max_string_len = 0;
    assert_eq!(
        parse_response_frame(&frame(&[101, b'x']), &limits).unwrap_err(),
        ResponseParseError::StringTooLarge
    );
    limits = defaults.clone();
    limits.max_array_len = 0;
    assert_eq!(
        parse_response_frame(&frame(&[17, 200, 255]), &limits).unwrap_err(),
        ResponseParseError::ArrayLimitExceeded
    );
    limits = defaults.clone();
    limits.max_hash_len = 0;
    assert_eq!(
        parse_response_frame(&frame(&[16, 100, 200, 255]), &limits).unwrap_err(),
        ResponseParseError::HashLimitExceeded
    );
    limits = defaults;
    limits.max_depth = 0;
    assert_eq!(
        parse_response_frame(&frame(&[17, 17, 255, 255]), &limits).unwrap_err(),
        ResponseParseError::NestingLimitExceeded
    );
}
