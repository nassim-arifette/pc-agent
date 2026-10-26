use crate::AgentError;

/// Submission scope: an explicit NVIDIA Nemotron ID, with no fallback to another vendor.
pub fn validate_model_id(id: &str) -> Result<(), AgentError> {
    let lower = id.to_ascii_lowercase();
    if id.len() > 200
        || !lower.starts_with("nvidia/")
        || !lower.contains("nemotron")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        return Err(AgentError::InvalidModel);
    }
    Ok(())
}
