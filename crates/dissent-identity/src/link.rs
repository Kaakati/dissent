//! Shareable links for postcard-encoded protocol values: `dissent://<kind>/<base32>`.

use crate::{IdentityError, Result};
use serde::{de::DeserializeOwned, Serialize};

pub(crate) fn encode<T: Serialize>(kind: &str, value: &T) -> Result<String> {
    let bytes = postcard::to_allocvec(value).map_err(|e| IdentityError::Serde(e.to_string()))?;
    let body = data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase();
    Ok(format!("dissent://{kind}/{body}"))
}

pub(crate) fn decode<T: DeserializeOwned>(kind: &str, link: &str) -> Result<T> {
    let prefix = format!("dissent://{kind}/");
    let body = link
        .trim()
        .strip_prefix(prefix.as_str())
        .ok_or_else(|| IdentityError::Encoding(format!("expected a {prefix} link")))?;
    let bytes = data_encoding::BASE32_NOPAD
        .decode(body.to_ascii_uppercase().as_bytes())
        .map_err(|e| IdentityError::Encoding(e.to_string()))?;
    postcard::from_bytes(&bytes).map_err(|e| IdentityError::Serde(e.to_string()))
}
