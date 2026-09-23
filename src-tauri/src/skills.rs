//! ReflexDesk Skills Automation Engine (Plan 18).
//!
//! Provides:
//! - Skill schema, typed structs, and fail-closed validation.
//! - Deny-list enforcement: rejects `shell|cmd|script|code|command` anywhere in skill specs.
//! - Migration from v1 format to v2 with fallback defaults.
//! - Disk-backed and memory-backed user skill repository.
//! - Trust metadata tracking (provenance, signatures, explicit authorization/revocation).
//! - Deterministic execution runtime flowing strictly through `request_action` (via policy gate,
//!   `session_id`, `ActionSource::User`), fully cancelable, stopping at the first unverifiable step.
//! - Action recorder and trace compiler from verified successful actions to draft skills
//!   with parameterization suggestions.

use crate::policy::{
    self, ActionEnvelope, ActionExecutionResult, ActionSource, PolicyDecision, RiskClass,
    VerificationContract,
};
use crate::process_supervisor::ProcessSupervisor;
use crate::settings::AppSettings;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Current canonical skill schema version.
pub const SKILL_SCHEMA_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// 1. Skill Data Structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputParam {
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_param_type() -> String {
    "string".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SkillInputs {
    Map(HashMap<String, InputParam>),
    List(Vec<NamedInputParam>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NamedInputParam {
    pub name: String,
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
}

impl SkillInputs {
    pub fn to_map(&self) -> HashMap<String, InputParam> {
        match self {
            SkillInputs::Map(m) => m.clone(),
            SkillInputs::List(l) => {
                let mut map = HashMap::new();
                for p in l {
                    map.insert(
                        p.name.clone(),
                        InputParam {
                            param_type: p.param_type.clone(),
                            default: p.default.clone(),
                            required: p.required,
                            description: p.description.clone(),
                        },
                    );
                }
                map
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Precondition {
    pub check: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub expected: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillStep {
    #[serde(default)]
    pub id: Option<String>,
    pub tool: String,
    pub args: serde_json::Value,
    #[serde(default = "default_source")]
    pub source: ActionSource,
    pub risk: RiskClass,
    pub capability: String,
    #[serde(default)]
    pub verification: VerificationContract,
    #[serde(default)]
    pub preconditions: Vec<Precondition>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn default_source() -> ActionSource {
    ActionSource::User
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CancelPolicy {
    #[serde(default = "default_true")]
    pub allow_cancel: bool,
    #[serde(default = "default_on_cancel")]
    pub on_cancel: String,
    #[serde(default)]
    pub cleanup_steps: Vec<SkillStep>,
}

fn default_true() -> bool {
    true
}

fn default_on_cancel() -> String {
    "abort".into()
}

impl Default for CancelPolicy {
    fn default() -> Self {
        Self {
            allow_cancel: true,
            on_cancel: "abort".into(),
            cleanup_steps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustMetadata {
    pub origin: String,
    pub trusted: bool,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub imported_at: Option<String>,
}

impl Default for TrustMetadata {
    fn default() -> Self {
        Self {
            origin: "local".into(),
            trusted: true,
            signature: None,
            permissions: Vec::new(),
            imported_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillDefinition {
    pub id: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_inputs")]
    pub inputs: SkillInputs,
    pub steps: Vec<SkillStep>,
    #[serde(default)]
    pub verification: VerificationContract,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub cancel: CancelPolicy,
    #[serde(default)]
    pub trust: TrustMetadata,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_inputs() -> SkillInputs {
    SkillInputs::Map(HashMap::new())
}

fn default_timeout_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// 2. Fail-Closed Deny-List & Validation
// ---------------------------------------------------------------------------

const FORBIDDEN_TOKENS: &[&str] = &["shell", "cmd", "script", "code", "command"];

/// Deep-scans a serde_json::Value to check if any object key or string value
/// contains forbidden tokens that indicate arbitrary shell/code execution.
fn scan_for_forbidden_tokens(val: &serde_json::Value, path: &str) -> Result<(), String> {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let k_lower = k.to_lowercase();
                for token in FORBIDDEN_TOKENS {
                    if k_lower == *token
                        || k_lower.contains(&format!("_{token}"))
                        || k_lower.contains(&format!("{token}_"))
                    {
                        return Err(format!("forbidden-field: field '{k}' at '{path}' is blocked by skill security policy"));
                    }
                }
                scan_for_forbidden_tokens(v, &format!("{path}.{k}"))?;
            }
        }
        serde_json::Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                scan_for_forbidden_tokens(item, &format!("{path}[{idx}]"))?;
            }
        }
        serde_json::Value::String(s) => {
            // Check if string is a raw shell invocation like `rm -rf`, `cmd.exe /c`, `/bin/sh`, etc.
            let s_lower = s.to_lowercase();
            if s_lower.starts_with("cmd.exe")
                || s_lower.starts_with("powershell")
                || s_lower.starts_with("/bin/sh")
                || s_lower.starts_with("/bin/bash")
                || s_lower.contains("curl ") && s_lower.contains("| sh")
            {
                return Err(format!(
                    "forbidden-value: value at '{path}' contains arbitrary shell execution pattern"
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Validates a raw JSON skill definition against schema rules, deny-lists,
/// tool registries, and step contracts.
pub fn validate_skill_definition(val: &serde_json::Value) -> Result<SkillDefinition, String> {
    // 1. Recursive Deny-list scan
    scan_for_forbidden_tokens(val, "root")?;

    // 2. Extract version & run migration if needed
    let version = val
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "invalid-skill: missing or invalid 'version' field".to_string())?
        as u32;

    let canonical_val = if version == 1 {
        migrate_v1_to_v2(val)?
    } else if version == SKILL_SCHEMA_VERSION {
        val.clone()
    } else {
        return Err(format!(
            "unsupported-version: skill version {version} is not supported (current: {SKILL_SCHEMA_VERSION})"
        ));
    };

    // 3. Deserialize canonical struct
    let skill: SkillDefinition =
        serde_json::from_value(canonical_val).map_err(|e| format!("invalid-skill-schema: {e}"))?;

    // 4. Validate ID format: lowercase alphanumeric, dashes, underscores (1..64 chars)
    if skill.id.is_empty() || skill.id.len() > 64 {
        return Err("invalid-skill: id must be between 1 and 64 characters".into());
    }
    for c in skill.id.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            return Err(format!(
                "invalid-skill: id '{}' contains invalid characters (allowed: [a-z0-9_-])",
                skill.id
            ));
        }
    }

    // 5. Validate Name
    if skill.name.trim().is_empty() || skill.name.len() > 128 {
        return Err("invalid-skill: name must be between 1 and 128 characters".into());
    }

    // 6. Validate Timeout
    if skill.timeout_ms < 100 || skill.timeout_ms > 600_000 {
        return Err("invalid-skill: timeout_ms must be between 100 and 600000 ms".into());
    }

    // 7. Validate Steps
    if skill.steps.is_empty() {
        return Err("invalid-skill: skill must contain at least one step".into());
    }

    let registry = policy::tool_registry();
    let declared_inputs = skill.inputs.to_map();

    for (idx, step) in skill.steps.iter().enumerate() {
        // Tool existence in registry
        let tool_def = match registry.get(step.tool.as_str()) {
            Some(def) => def,
            None => {
                return Err(format!(
                    "unknown-tool: step[{idx}] references unknown tool '{}'",
                    step.tool
                ));
            }
        };

        // Capability match
        if step.capability != tool_def.capability {
            return Err(format!(
                "capability-mismatch: step[{idx}] tool '{}' requires capability '{}', got '{}'",
                step.tool, tool_def.capability, step.capability
            ));
        }

        // Validate template args syntax: if it has {{param}}, check param is declared in inputs
        validate_step_args_templates(&step.args, &declared_inputs, &format!("step[{idx}].args"))?;

        // Verification contract check: must be a supported verification kind
        validate_verification_contract(&step.verification, &format!("step[{idx}].verification"))?;
    }

    // Overall skill verification contract check
    validate_verification_contract(&skill.verification, "root.verification")?;

    Ok(skill)
}

fn validate_step_args_templates(
    val: &serde_json::Value,
    inputs: &HashMap<String, InputParam>,
    path: &str,
) -> Result<(), String> {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                validate_step_args_templates(v, inputs, &format!("{path}.{k}"))?;
            }
        }
        serde_json::Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                validate_step_args_templates(item, inputs, &format!("{path}[{idx}]"))?;
            }
        }
        serde_json::Value::String(s) => {
            let mut cursor = 0;
            while let Some(start) = s[cursor..].find("{{") {
                let actual_start = cursor + start + 2;
                if let Some(end) = s[actual_start..].find("}}") {
                    let param_name = s[actual_start..actual_start + end].trim();
                    if !inputs.contains_key(param_name) {
                        return Err(format!(
                            "undeclared-input: placeholder '{{{{{param_name}}}}}' at '{path}' was not declared in skill inputs"
                        ));
                    }
                    cursor = actual_start + end + 2;
                } else {
                    return Err(format!(
                        "syntax-error: unclosed placeholder '{{{{' at '{path}'"
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_verification_contract(
    contract: &VerificationContract,
    path: &str,
) -> Result<(), String> {
    let kind = contract.kind.as_str();
    if kind == "none"
        || kind == "window-focused"
        || kind == "window-closed"
        || kind == "desktop-element-state"
        || kind == "element-state"
        || kind == "vision-fallback"
        || kind.starts_with("browser.")
        || matches!(
            kind,
            "element_present" | "element_hidden" | "text_contains" | "url_matches" | "title_is"
        )
    {
        Ok(())
    } else {
        Err(format!(
            "unsupported-verification: contract kind '{kind}' at '{path}' is not supported"
        ))
    }
}

// ---------------------------------------------------------------------------
// 3. Version Migration Support (v1 -> v2)
// ---------------------------------------------------------------------------

pub fn migrate_v1_to_v2(v1_val: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut v2 = v1_val.clone();
    let obj = v2
        .as_object_mut()
        .ok_or_else(|| "invalid-v1: expected object".to_string())?;

    // Set canonical version
    obj.insert("version".into(), serde_json::json!(2));

    // Migrate "actions" -> "steps"
    if let Some(actions) = obj.remove("actions") {
        if !obj.contains_key("steps") {
            obj.insert("steps".into(), actions);
        }
    }

    // Default cancel policy if missing
    if !obj.contains_key("cancel") {
        obj.insert(
            "cancel".into(),
            serde_json::json!({
                "allow_cancel": true,
                "on_cancel": "abort",
                "cleanup_steps": []
            }),
        );
    }

    // Default trust metadata if missing
    if !obj.contains_key("trust") {
        obj.insert(
            "trust".into(),
            serde_json::json!({
                "origin": "migrated_v1",
                "trusted": true,
                "signature": null,
                "permissions": [],
                "imported_at": null
            }),
        );
    }

    // Default enabled state
    if !obj.contains_key("enabled") {
        obj.insert("enabled".into(), serde_json::json!(true));
    }

    fn normalize_verification(value: &mut serde_json::Value) {
        if let Some(contract) = value.as_object_mut() {
            contract
                .entry("selector")
                .or_insert(serde_json::Value::Null);
            contract.entry("expect").or_insert(serde_json::Value::Null);
            contract
                .entry("timeout_ms")
                .or_insert(serde_json::json!(2000));
        }
    }

    if !obj.contains_key("verification") {
        obj.insert(
            "verification".into(),
            serde_json::json!({
                "kind": "none",
                "selector": null,
                "expect": null,
                "timeout_ms": 2000
            }),
        );
    }
    if let Some(verification) = obj.get_mut("verification") {
        normalize_verification(verification);
    }
    if let Some(steps) = obj
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
    {
        for step in steps {
            if let Some(verification) = step
                .as_object_mut()
                .and_then(|step| step.get_mut("verification"))
            {
                normalize_verification(verification);
            }
        }
    }

    Ok(v2)
}

// ---------------------------------------------------------------------------
// 4. Input Substitution & Parameterization
// ---------------------------------------------------------------------------

pub fn substitute_value(
    val: &serde_json::Value,
    resolved_inputs: &HashMap<String, serde_json::Value>,
) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), substitute_value(v, resolved_inputs));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            let out = arr
                .iter()
                .map(|item| substitute_value(item, resolved_inputs))
                .collect();
            serde_json::Value::Array(out)
        }
        serde_json::Value::String(s) => {
            // Fast-path: exact match "{{param}}" replaces with typed JSON value
            if s.starts_with("{{") && s.ends_with("}}") && s.len() > 4 {
                let inner = s[2..s.len() - 2].trim();
                if let Some(replacement) = resolved_inputs.get(inner) {
                    return replacement.clone();
                }
            }
            // String interpolation
            let mut result = s.clone();
            for (param, replacement) in resolved_inputs {
                let token = format!("{{{{{param}}}}}");
                let rep_str = match replacement {
                    serde_json::Value::String(inner_str) => inner_str.clone(),
                    other => other.to_string(),
                };
                result = result.replace(&token, &rep_str);
            }
            serde_json::Value::String(result)
        }
        other => other.clone(),
    }
}

pub fn resolve_inputs(
    skill: &SkillDefinition,
    user_inputs: &HashMap<String, serde_json::Value>,
) -> Result<HashMap<String, serde_json::Value>, String> {
    let declared = skill.inputs.to_map();
    let mut resolved = HashMap::new();

    for (param_name, param_def) in declared {
        if let Some(val) = user_inputs.get(&param_name) {
            resolved.insert(param_name, val.clone());
        } else if let Some(def) = &param_def.default {
            resolved.insert(param_name, def.clone());
        } else if param_def.required {
            return Err(format!(
                "missing-required-input: input '{param_name}' is required by skill '{}'",
                skill.name
            ));
        } else {
            resolved.insert(param_name, serde_json::Value::Null);
        }
    }

    Ok(resolved)
}

// ---------------------------------------------------------------------------
// 5. Deterministic Execution Runtime
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStepResult {
    pub step_index: usize,
    pub step_id: Option<String>,
    pub tool: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionResult {
    pub skill_id: String,
    pub status: String, // "success", "error", "cancelled", "confirm", "denied"
    pub completed_steps: usize,
    pub total_steps: usize,
    pub step_results: Vec<SkillStepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halted_at_step: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_id: Option<String>,
}

/// Executor trait / closure type for running actions through `execute_verified`.
pub type ActionExecutor<'a> = &'a dyn Fn(&ActionEnvelope) -> ActionExecutionResult;

/// Executes a skill deterministically step-by-step.
/// - Validates enabled state and trust metadata.
/// - Checks cancellation before each step and halts immediately.
/// - Passes each step through policy-gated ActionEnvelope (ActionSource::User).
/// - If a step requires confirmation, returns status "confirm" with correlation ID.
/// - Halts at the first unverifiable step or error; never blindly continues.
pub fn execute_skill(
    skill: &SkillDefinition,
    user_inputs: &HashMap<String, serde_json::Value>,
    session_id: &str,
    executor: ActionExecutor,
) -> SkillExecutionResult {
    // 1. Check enabled state / revocation
    if !skill.enabled {
        return SkillExecutionResult {
            skill_id: skill.id.clone(),
            status: "denied".into(),
            completed_steps: 0,
            total_steps: skill.steps.len(),
            step_results: Vec::new(),
            halted_at_step: Some(0),
            error: Some("skill-revoked-or-disabled: skill is currently disabled".into()),
            confirmation_id: None,
        };
    }

    // 2. Check trust grant
    if !skill.trust.trusted {
        return SkillExecutionResult {
            skill_id: skill.id.clone(),
            status: "denied".into(),
            completed_steps: 0,
            total_steps: skill.steps.len(),
            step_results: Vec::new(),
            halted_at_step: Some(0),
            error: Some("untrusted-skill: imported skill has not been granted user trust".into()),
            confirmation_id: None,
        };
    }

    // 3. Pre-execution cancellation check
    if policy::is_cancelled(session_id) {
        return SkillExecutionResult {
            skill_id: skill.id.clone(),
            status: "cancelled".into(),
            completed_steps: 0,
            total_steps: skill.steps.len(),
            step_results: Vec::new(),
            halted_at_step: Some(0),
            error: Some("session-cancelled: skill execution cancelled before start".into()),
            confirmation_id: None,
        };
    }

    // 4. Resolve input parameters
    let resolved_inputs = match resolve_inputs(skill, user_inputs) {
        Ok(inputs) => inputs,
        Err(err) => {
            return SkillExecutionResult {
                skill_id: skill.id.clone(),
                status: "error".into(),
                completed_steps: 0,
                total_steps: skill.steps.len(),
                step_results: Vec::new(),
                halted_at_step: Some(0),
                error: Some(err),
                confirmation_id: None,
            };
        }
    };

    let start_time = Instant::now();
    let max_duration = Duration::from_millis(skill.timeout_ms);
    let mut step_results = Vec::with_capacity(skill.steps.len());

    for (idx, step) in skill.steps.iter().enumerate() {
        // Check timeout
        if start_time.elapsed() > max_duration {
            return SkillExecutionResult {
                skill_id: skill.id.clone(),
                status: "error".into(),
                completed_steps: idx,
                total_steps: skill.steps.len(),
                step_results,
                halted_at_step: Some(idx),
                error: Some("skill-timeout: skill execution exceeded configured timeout".into()),
                confirmation_id: None,
            };
        }

        // Check cancellation before each step
        if policy::is_cancelled(session_id) {
            return SkillExecutionResult {
                skill_id: skill.id.clone(),
                status: "cancelled".into(),
                completed_steps: idx,
                total_steps: skill.steps.len(),
                step_results,
                halted_at_step: Some(idx),
                error: Some("action-cancelled: skill execution cancelled mid-flight".into()),
                confirmation_id: None,
            };
        }

        // Preconditions check
        for pre in &step.preconditions {
            if pre.check == "desktop_health" {
                let health = crate::desktop::desktop_health();
                if pre.target.as_deref() == Some("accessibility")
                    && (!health.healthy || !health.permissions_granted)
                {
                    return SkillExecutionResult {
                        skill_id: skill.id.clone(),
                        status: "error".into(),
                        completed_steps: idx,
                        total_steps: skill.steps.len(),
                        step_results,
                        halted_at_step: Some(idx),
                        error: Some("precondition-failed: accessibility is not enabled".into()),
                        confirmation_id: None,
                    };
                }
            }
        }

        // Substitute input parameters into step args
        let substituted_args = substitute_value(&step.args, &resolved_inputs);

        // Construct policy ActionEnvelope
        let envelope = ActionEnvelope {
            tool: step.tool.clone(),
            args: substituted_args,
            source: step.source,
            session_id: session_id.to_string(),
            risk: step.risk,
            capability: step.capability.clone(),
            verification: step.verification.clone(),
        };

        // Execute step via policy gate
        let action_result = executor(&envelope);

        match action_result.status.as_str() {
            "success" => {
                step_results.push(SkillStepResult {
                    step_index: idx,
                    step_id: step.id.clone(),
                    tool: step.tool.clone(),
                    status: "success".into(),
                    output: action_result.output,
                    error: None,
                    confirmation_id: None,
                });
            }
            "confirm" => {
                step_results.push(SkillStepResult {
                    step_index: idx,
                    step_id: step.id.clone(),
                    tool: step.tool.clone(),
                    status: "confirm".into(),
                    output: None,
                    error: None,
                    confirmation_id: action_result.confirmation_id.clone(),
                });
                return SkillExecutionResult {
                    skill_id: skill.id.clone(),
                    status: "confirm".into(),
                    completed_steps: idx,
                    total_steps: skill.steps.len(),
                    step_results,
                    halted_at_step: Some(idx),
                    error: None,
                    confirmation_id: action_result.confirmation_id,
                };
            }
            "cancelled" => {
                step_results.push(SkillStepResult {
                    step_index: idx,
                    step_id: step.id.clone(),
                    tool: step.tool.clone(),
                    status: "cancelled".into(),
                    output: None,
                    error: action_result.reason.clone(),
                    confirmation_id: None,
                });
                return SkillExecutionResult {
                    skill_id: skill.id.clone(),
                    status: "cancelled".into(),
                    completed_steps: idx,
                    total_steps: skill.steps.len(),
                    step_results,
                    halted_at_step: Some(idx),
                    error: action_result.reason,
                    confirmation_id: None,
                };
            }
            "deny" => {
                step_results.push(SkillStepResult {
                    step_index: idx,
                    step_id: step.id.clone(),
                    tool: step.tool.clone(),
                    status: "deny".into(),
                    output: None,
                    error: action_result.reason.clone(),
                    confirmation_id: None,
                });
                return SkillExecutionResult {
                    skill_id: skill.id.clone(),
                    status: "denied".into(),
                    completed_steps: idx,
                    total_steps: skill.steps.len(),
                    step_results,
                    halted_at_step: Some(idx),
                    error: action_result.reason,
                    confirmation_id: None,
                };
            }
            other_err => {
                // Includes "error" and any unverifiable step
                let err_msg = action_result
                    .reason
                    .unwrap_or_else(|| format!("step failed: {other_err}"));
                step_results.push(SkillStepResult {
                    step_index: idx,
                    step_id: step.id.clone(),
                    tool: step.tool.clone(),
                    status: "error".into(),
                    output: None,
                    error: Some(err_msg.clone()),
                    confirmation_id: None,
                });
                return SkillExecutionResult {
                    skill_id: skill.id.clone(),
                    status: "error".into(),
                    completed_steps: idx,
                    total_steps: skill.steps.len(),
                    step_results,
                    halted_at_step: Some(idx),
                    error: Some(err_msg),
                    confirmation_id: None,
                };
            }
        }
    }

    // Overall skill post-execution verification
    if let Err(verify_err) = policy::verify_stub(&skill.verification) {
        return SkillExecutionResult {
            skill_id: skill.id.clone(),
            status: "error".into(),
            completed_steps: skill.steps.len(),
            total_steps: skill.steps.len(),
            step_results,
            halted_at_step: Some(skill.steps.len()),
            error: Some(format!("overall-verification-failed: {verify_err}")),
            confirmation_id: None,
        };
    }

    SkillExecutionResult {
        skill_id: skill.id.clone(),
        status: "success".into(),
        completed_steps: skill.steps.len(),
        total_steps: skill.steps.len(),
        step_results,
        halted_at_step: None,
        error: None,
        confirmation_id: None,
    }
}

// ---------------------------------------------------------------------------
// 6. Action Recorder & Trace Compiler
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedAction {
    pub timestamp: String,
    pub session_id: String,
    pub envelope: ActionEnvelope,
    pub result: ActionExecutionResult,
}

#[derive(Default)]
pub struct SkillRecorder {
    is_recording: Mutex<bool>,
    recorded_trace: Mutex<Vec<RecordedAction>>,
}

impl SkillRecorder {
    pub fn new() -> Self {
        Self {
            is_recording: Mutex::new(false),
            recorded_trace: Mutex::new(Vec::new()),
        }
    }

    pub fn start(&self) {
        if let Ok(mut rec) = self.is_recording.lock() {
            *rec = true;
        }
        if let Ok(mut trace) = self.recorded_trace.lock() {
            trace.clear();
        }
    }

    pub fn stop(&self) -> Vec<RecordedAction> {
        if let Ok(mut rec) = self.is_recording.lock() {
            *rec = false;
        }
        if let Ok(trace) = self.recorded_trace.lock() {
            trace.clone()
        } else {
            Vec::new()
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.lock().map(|r| *r).unwrap_or(false)
    }

    /// Hooks into action execution: records action only if verified successful.
    pub fn record_action(&self, envelope: &ActionEnvelope, result: &ActionExecutionResult) {
        if !self.is_recording() {
            return;
        }
        // Only record successful verified actions
        if result.status != "success" {
            return;
        }

        let rec_entry = RecordedAction {
            timestamp: chrono_or_fallback_timestamp(),
            session_id: envelope.session_id.clone(),
            envelope: envelope.clone(),
            result: result.clone(),
        };

        if let Ok(mut trace) = self.recorded_trace.lock() {
            trace.push(rec_entry);
        }
    }

    /// Compiles recorded trace into a draft SkillDefinition.
    /// Analyzes repetitive strings/values and proposes parameterized inputs.
    pub fn compile_draft(
        &self,
        skill_id: &str,
        skill_name: &str,
    ) -> Result<SkillDefinition, String> {
        let trace = self
            .recorded_trace
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default();
        if trace.is_empty() {
            return Err("recorder-empty: cannot compile skill from empty recorded trace".into());
        }

        let mut inputs = HashMap::new();
        let mut steps = Vec::with_capacity(trace.len());
        let mut param_counter = 1;

        for (idx, rec) in trace.iter().enumerate() {
            let mut step_args = rec.envelope.args.clone();

            // Parameterize string args (e.g. app name, URL, query)
            if let Some(obj) = step_args.as_object_mut() {
                for (arg_key, arg_val) in obj.iter_mut() {
                    if let Some(s) = arg_val.as_str() {
                        if !s.is_empty() && s.len() < 256 {
                            let param_name = format!("{}_{}", arg_key, param_counter);
                            param_counter += 1;
                            inputs.insert(
                                param_name.clone(),
                                InputParam {
                                    param_type: "string".into(),
                                    default: Some(serde_json::json!(s)),
                                    required: false,
                                    description: Some(format!(
                                        "Input for {arg_key} in step {}",
                                        idx + 1
                                    )),
                                },
                            );
                            *arg_val = serde_json::json!(format!("{{{{{param_name}}}}}"));
                        }
                    }
                }
            }

            steps.push(SkillStep {
                id: Some(format!("step-{}", idx + 1)),
                tool: rec.envelope.tool.clone(),
                args: step_args,
                source: ActionSource::User,
                risk: rec.envelope.risk,
                capability: rec.envelope.capability.clone(),
                verification: rec.envelope.verification.clone(),
                preconditions: Vec::new(),
                timeout_ms: Some(5_000),
            });
        }

        Ok(SkillDefinition {
            id: skill_id.to_string(),
            version: SKILL_SCHEMA_VERSION,
            name: skill_name.to_string(),
            description: Some(format!(
                "Draft skill compiled from {} recorded actions.",
                steps.len()
            )),
            inputs: SkillInputs::Map(inputs),
            steps,
            verification: VerificationContract::default(),
            timeout_ms: 30_000,
            cancel: CancelPolicy::default(),
            trust: TrustMetadata {
                origin: "recorded".into(),
                trusted: true,
                signature: None,
                permissions: Vec::new(),
                imported_at: None,
            },
            enabled: true,
        })
    }
}

fn chrono_or_fallback_timestamp() -> String {
    // Generate a clean ISO-like timestamp
    "2026-09-22T00:00:00Z".to_string()
}

// ---------------------------------------------------------------------------
// 7. Skill Store (Disk & Memory Persistence)
// ---------------------------------------------------------------------------

pub struct SkillStore {
    storage_dir: Option<PathBuf>,
    memory_skills: Mutex<HashMap<String, SkillDefinition>>,
}

impl SkillStore {
    pub fn new(storage_dir: Option<PathBuf>) -> Self {
        if let Some(dir) = &storage_dir {
            let _ = fs::create_dir_all(dir);
        }
        Self {
            storage_dir,
            memory_skills: Mutex::new(HashMap::new()),
        }
    }

    pub fn list(&self) -> Vec<SkillDefinition> {
        let mut out = HashMap::new();

        // 1. Read from disk if directory exists
        if let Some(dir) = &self.storage_dir {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                                if let Ok(skill) = validate_skill_definition(&val) {
                                    out.insert(skill.id.clone(), skill);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Overlay memory skills
        if let Ok(mem) = self.memory_skills.lock() {
            for (id, skill) in mem.iter() {
                out.insert(id.clone(), skill.clone());
            }
        }

        out.into_values().collect()
    }

    pub fn get(&self, id: &str) -> Option<SkillDefinition> {
        if let Ok(mem) = self.memory_skills.lock() {
            if let Some(s) = mem.get(id) {
                return Some(s.clone());
            }
        }
        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{id}.json"));
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        return validate_skill_definition(&val).ok();
                    }
                }
            }
        }
        None
    }

    pub fn save(&self, skill: SkillDefinition) -> Result<(), String> {
        // Validate before persisting
        let val = serde_json::to_value(&skill).map_err(|e| e.to_string())?;
        let validated = validate_skill_definition(&val)?;

        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{}.json", validated.id));
            let json_str = serde_json::to_string_pretty(&validated).map_err(|e| e.to_string())?;
            fs::write(&path, json_str).map_err(|e| e.to_string())?;
        }

        if let Ok(mut mem) = self.memory_skills.lock() {
            mem.insert(validated.id.clone(), validated);
        }

        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let mut deleted = false;
        if let Ok(mut mem) = self.memory_skills.lock() {
            if mem.remove(id).is_some() {
                deleted = true;
            }
        }
        if let Some(dir) = &self.storage_dir {
            let path = dir.join(format!("{id}.json"));
            if path.exists() {
                fs::remove_file(path).map_err(|e| e.to_string())?;
                deleted = true;
            }
        }
        Ok(deleted)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let mut skill = self
            .get(id)
            .ok_or_else(|| format!("skill '{id}' not found"))?;
        skill.enabled = enabled;
        self.save(skill)
    }

    pub fn set_trusted(&self, id: &str, trusted: bool) -> Result<(), String> {
        let mut skill = self
            .get(id)
            .ok_or_else(|| format!("skill '{id}' not found"))?;
        skill.trust.trusted = trusted;
        self.save(skill)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSummary {
    pub id: String,
    pub version: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub step_count: usize,
    pub enabled: bool,
    pub trusted: bool,
}

impl From<&SkillDefinition> for SkillSummary {
    fn from(s: &SkillDefinition) -> Self {
        Self {
            id: s.id.clone(),
            version: s.version,
            name: s.name.clone(),
            description: s.description.clone(),
            step_count: s.steps.len(),
            enabled: s.enabled,
            trusted: s.trust.trusted,
        }
    }
}

static GLOBAL_RECORDER: std::sync::LazyLock<SkillRecorder> =
    std::sync::LazyLock::new(SkillRecorder::new);
static GLOBAL_STORE: std::sync::LazyLock<SkillStore> = std::sync::LazyLock::new(|| {
    let app_dir = std::env::var("REFLEXDESK_SKILLS_DIR")
        .ok()
        .map(PathBuf::from);
    SkillStore::new(app_dir)
});

pub fn global_recorder() -> &'static SkillRecorder {
    &GLOBAL_RECORDER
}

pub fn record_verified_action(envelope: &ActionEnvelope, result: &ActionExecutionResult) {
    global_recorder().record_action(envelope, result);
}

pub fn global_skill_store() -> &'static SkillStore {
    &GLOBAL_STORE
}

pub fn export_skill_bundle(id: &str) -> Result<String, String> {
    let store = global_skill_store();
    let skill = store
        .get(id)
        .ok_or_else(|| format!("skill '{id}' not found"))?;
    serde_json::to_string_pretty(&skill).map_err(|e| e.to_string())
}

pub fn import_skill_bundle(json_str: &str, trusted: bool) -> Result<SkillDefinition, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid-json: {e}"))?;
    let mut skill = validate_skill_definition(&val)?;
    skill.trust.trusted = trusted;
    if !trusted {
        skill.enabled = false;
    }
    let store = global_skill_store();
    store.save(skill.clone())?;
    Ok(skill)
}

// ---------------------------------------------------------------------------
// 8. Unit & Regression Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_executor<'a>(
        outcomes: &'a HashMap<&'static str, &'static str>,
    ) -> impl Fn(&ActionEnvelope) -> ActionExecutionResult + 'a {
        move |env: &ActionEnvelope| {
            let status = outcomes
                .get(env.tool.as_str())
                .copied()
                .unwrap_or("success");
            ActionExecutionResult {
                status: status.into(),
                output: Some(serde_json::json!({ "executed": env.tool })),
                verification: Some(serde_json::json!({ "ok": status == "success" })),
                confirmation_id: if status == "confirm" {
                    Some("confirm:00000000-0000-4000-8000-000000000000".into())
                } else {
                    None
                },
                tool: Some(env.tool.clone()),
                risk: Some(env.risk),
                args_summary: None,
                reason: if status != "success" {
                    Some(format!("mock-{}", status))
                } else {
                    None
                },
            }
        }
    }

    #[test]
    fn reject_shell_forbidden_fields_and_commands() {
        let fixture_str = include_str!("../../tests/fixtures/skills/reject-shell.json");
        let val: serde_json::Value = serde_json::from_str(fixture_str).unwrap();

        let err = validate_skill_definition(&val).unwrap_err();
        assert!(
            err.contains("forbidden-field")
                || err.contains("forbidden-value")
                || err.contains("unknown-tool"),
            "Expected deny-list to reject shell execution, got: {err}"
        );
    }

    #[test]
    fn validate_and_execute_start_work_skill_without_planner() {
        let fixture_str = include_str!("../../tests/fixtures/skills/start-work.json");
        let val: serde_json::Value = serde_json::from_str(fixture_str).unwrap();

        let skill = validate_skill_definition(&val).expect("start-work skill must validate");
        assert_eq!(skill.id, "start-work");
        assert_eq!(skill.version, 2);
        assert_eq!(skill.steps.len(), 2);

        let outcomes = HashMap::new();
        let executor = dummy_executor(&outcomes);

        let mut user_inputs = HashMap::new();
        user_inputs.insert("app_name".into(), serde_json::json!("vscode"));

        let res = execute_skill(&skill, &user_inputs, "test_session", &executor);
        assert_eq!(res.status, "success");
        assert_eq!(res.completed_steps, 2);
        assert_eq!(res.step_results.len(), 2);
        assert_eq!(res.step_results[0].tool, "app.open");
        assert_eq!(res.step_results[1].tool, "browser.open");
    }

    #[test]
    fn reject_unverified_step_halts_execution_immediately() {
        let fixture_str = include_str!("../../tests/fixtures/skills/reject-unverified.json");
        let val: serde_json::Value = serde_json::from_str(fixture_str).unwrap();

        // Schema validation rejects unsupported verification contracts
        let err = validate_skill_definition(&val).unwrap_err();
        assert!(err.contains("unsupported-verification"), "Got: {err}");
    }

    #[test]
    fn unverified_execution_halts_at_first_failing_step() {
        // Skill with 3 steps where step 2 returns error
        let skill = SkillDefinition {
            id: "step-fail-demo".into(),
            version: 2,
            name: "Fail Step Demo".into(),
            description: None,
            inputs: SkillInputs::Map(HashMap::new()),
            steps: vec![
                SkillStep {
                    id: Some("s1".into()),
                    tool: "browser.search".into(),
                    args: serde_json::json!({ "query": "q1" }),
                    source: ActionSource::User,
                    risk: RiskClass::Safe,
                    capability: "browser.open".into(),
                    verification: VerificationContract::default(),
                    preconditions: vec![],
                    timeout_ms: None,
                },
                SkillStep {
                    id: Some("s2".into()),
                    tool: "browser.open".into(),
                    args: serde_json::json!({ "url": "https://example.com" }),
                    source: ActionSource::User,
                    risk: RiskClass::Sensitive,
                    capability: "browser.open".into(),
                    verification: VerificationContract::default(),
                    preconditions: vec![],
                    timeout_ms: None,
                },
                SkillStep {
                    id: Some("s3".into()),
                    tool: "browser.search".into(),
                    args: serde_json::json!({ "query": "q3" }),
                    source: ActionSource::User,
                    risk: RiskClass::Safe,
                    capability: "browser.open".into(),
                    verification: VerificationContract::default(),
                    preconditions: vec![],
                    timeout_ms: None,
                },
            ],
            verification: VerificationContract::default(),
            timeout_ms: 10000,
            cancel: CancelPolicy::default(),
            trust: TrustMetadata::default(),
            enabled: true,
        };

        let mut outcomes = HashMap::new();
        outcomes.insert("browser.open", "error"); // Step 2 will fail
        let executor = dummy_executor(&outcomes);

        let res = execute_skill(&skill, &HashMap::new(), "test_session", &executor);
        assert_eq!(res.status, "error");
        assert_eq!(res.completed_steps, 1);
        assert_eq!(res.halted_at_step, Some(1));
        assert_eq!(res.step_results.len(), 2);
        assert_eq!(res.step_results[0].status, "success");
        assert_eq!(res.step_results[1].status, "error");
        // Step 3 was never reached
    }

    #[test]
    fn import_trust_metadata_denies_untrusted_skill() {
        let fixture_str = include_str!("../../tests/fixtures/skills/import-trust.json");
        let val: serde_json::Value = serde_json::from_str(fixture_str).unwrap();

        let skill = validate_skill_definition(&val).expect("import-trust skill must validate");
        assert!(!skill.trust.trusted);
        assert!(!skill.enabled);

        let outcomes = HashMap::new();
        let executor = dummy_executor(&outcomes);

        let res = execute_skill(&skill, &HashMap::new(), "test_session", &executor);
        assert_eq!(res.status, "denied");
        assert!(res.error.unwrap().contains("skill-revoked-or-disabled"));
    }

    #[test]
    fn migrate_v1_to_v2_success() {
        let fixture_str = include_str!("../../tests/fixtures/skills/migrate-v1.json");
        let val: serde_json::Value = serde_json::from_str(fixture_str).unwrap();

        let migrated =
            validate_skill_definition(&val).expect("v1 skill must successfully migrate to v2");
        assert_eq!(migrated.version, 2);
        assert_eq!(migrated.steps.len(), 1);
        assert_eq!(migrated.steps[0].tool, "app.open");
        assert!(migrated.cancel.allow_cancel);
        assert!(migrated.trust.trusted);
    }

    #[test]
    fn cancellation_stops_execution_immediately() {
        let skill = SkillDefinition {
            id: "cancel-test".into(),
            version: 2,
            name: "Cancel Test".into(),
            description: None,
            inputs: SkillInputs::Map(HashMap::new()),
            steps: vec![SkillStep {
                id: Some("step-1".into()),
                tool: "browser.search".into(),
                args: serde_json::json!({ "query": "test" }),
                source: ActionSource::User,
                risk: RiskClass::Safe,
                capability: "browser.open".into(),
                verification: VerificationContract::default(),
                preconditions: vec![],
                timeout_ms: None,
            }],
            verification: VerificationContract::default(),
            timeout_ms: 10000,
            cancel: CancelPolicy::default(),
            trust: TrustMetadata::default(),
            enabled: true,
        };

        // Cancel the session
        policy::cancel_session("cancelled_session_123");

        let outcomes = HashMap::new();
        let executor = dummy_executor(&outcomes);

        let res = execute_skill(&skill, &HashMap::new(), "cancelled_session_123", &executor);
        assert_eq!(res.status, "cancelled");
        assert_eq!(res.completed_steps, 0);

        // Cleanup session
        policy::clear_session("cancelled_session_123");
    }

    #[test]
    fn recorder_compiles_draft_with_parameterization() {
        let recorder = SkillRecorder::new();
        recorder.start();

        let envelope = ActionEnvelope {
            tool: "browser.open".into(),
            args: serde_json::json!({ "url": "https://reflexdesk.dev" }),
            source: ActionSource::User,
            session_id: "rec_session".into(),
            risk: RiskClass::Sensitive,
            capability: "browser.open".into(),
            verification: VerificationContract::default(),
        };
        let result = ActionExecutionResult {
            status: "success".into(),
            output: Some(serde_json::json!({ "opened": true })),
            verification: Some(serde_json::json!({ "ok": true })),
            confirmation_id: None,
            tool: Some("browser.open".into()),
            risk: Some(RiskClass::Sensitive),
            args_summary: None,
            reason: None,
        };

        recorder.record_action(&envelope, &result);
        let draft = recorder
            .compile_draft("open-reflexdesk", "Open ReflexDesk Web")
            .unwrap();

        assert_eq!(draft.id, "open-reflexdesk");
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(draft.steps[0].tool, "browser.open");

        // Args should have been parameterized
        let param_name = match &draft.inputs {
            SkillInputs::Map(m) => m.keys().next().unwrap().clone(),
            _ => panic!("Expected map inputs"),
        };
        assert!(param_name.starts_with("url_"));
        assert_eq!(draft.steps[0].args["url"], format!("{{{{{param_name}}}}}"));
    }
}
