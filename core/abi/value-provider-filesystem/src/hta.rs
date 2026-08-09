fn exact_fields(request: Node<'_, '_>, expected: &[&str]) -> Result<(), Error> {
    if request.kind() != Kind::Map || request.len()? != expected.len() {
        return Err(Error::InvalidRequest("request fields are not exact"));
    }
    let mut seen = Vec::with_capacity(expected.len());
    for index in 0..request.len()? {
        let (key, _) = request.pair(index)?;
        if !matches!(key.kind(), Kind::String | Kind::Keyword) {
            return Err(Error::InvalidRequest(
                "request keys must be strings or keywords",
            ));
        }
        let key = key.as_text()?;
        if !expected.contains(&key) || seen.contains(&key) {
            return Err(Error::InvalidRequest(
                "request contains unknown or duplicate fields",
            ));
        }
        seen.push(key);
    }
    Ok(())
}

fn request_text<'a>(request: Node<'_, 'a>, name: &str) -> Result<&'a str, Error> {
    let value = request.require(name)?;
    if value.kind() != Kind::String {
        return Err(Error::InvalidRequest("text fields must be strings"));
    }
    Ok(value.as_text()?)
}

fn request_usize(request: Node<'_, '_>, name: &str, positive: bool) -> Result<usize, Error> {
    let value = request.require(name)?.as_i64()?;
    let value = usize::try_from(value)
        .map_err(|_| Error::InvalidRequest("integer fields must be non-negative"))?;
    if positive && value == 0 {
        return Err(Error::InvalidRequest("integer field must be positive"));
    }
    Ok(value)
}

fn classify_hta_error(message: &str) -> &'static str {
    if message.starts_with("hta/frame-noncanonical:") {
        HTA_NONCANONICAL
    } else if message.starts_with("hta/value-unsupported:") {
        VALUE_UNSUPPORTED
    } else if message.starts_with("hta/frame-too-large:")
        || message.starts_with("hta/maximum-invalid:")
    {
        MAXIMUM_EXCEEDED
    } else {
        HTA_INVALID
    }
}

fn success_result(digest: &str, value_frame: &[u8]) -> Result<Vec<u8>, Error> {
    let value = value_frame.strip_prefix(MAGIC).ok_or(Error::InvalidRequest(
        "verified value frame does not contain HTA1 magic",
    ))?;
    result_map(vec![
        ("byte-length", bare_usize(value_frame.len())?),
        ("digest", bare_string(digest)),
        ("operation", bare_string(OPERATION)),
        ("profile", bare_string(PROFILE)),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("value", value.to_vec()),
        ("verified", bare_bool(true)),
    ])
}

fn failure_result(digest: &str, code: &str) -> Result<Vec<u8>, Error> {
    result_map(vec![
        ("code", bare_string(code)),
        ("digest", bare_string(digest)),
        ("operation", bare_string(OPERATION)),
        ("protocol", bare_string(RESULT_PROTOCOL)),
        ("verified", bare_bool(false)),
    ])
}

fn result_map(entries: Vec<(&str, Vec<u8>)>) -> Result<Vec<u8>, Error> {
    let mut entries = entries
        .into_iter()
        .map(|(key, value)| (bare_keyword(key), value))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let count = u32::try_from(entries.len())
        .map_err(|_| Error::InvalidRequest("too many result fields"))?;
    let mut output = MAGIC.to_vec();
    output.push(MAP);
    output.extend_from_slice(&count.to_be_bytes());
    for (key, value) in entries {
        output.extend_from_slice(&key);
        output.extend_from_slice(&value);
    }
    Ok(output)
}

fn bare_keyword(value: &str) -> Vec<u8> {
    bare_text(KEYWORD, value)
}

fn bare_string(value: &str) -> Vec<u8> {
    bare_text(STRING, value)
}

fn bare_text(tag: u8, value: &str) -> Vec<u8> {
    let mut output = vec![tag];
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
    output
}

fn bare_bool(value: bool) -> Vec<u8> {
    vec![if value { TRUE } else { FALSE }]
}

fn bare_usize(value: usize) -> Result<Vec<u8>, Error> {
    let value = i64::try_from(value)
        .map_err(|_| Error::InvalidRequest("integer result exceeds signed 64-bit range"))?;
    let mut output = vec![I64];
    output.extend_from_slice(&value.to_be_bytes());
    Ok(output)
}
