use super::*;

#[derive(Debug, Clone)]
pub(crate) struct HttpGetFn {
    responses: BTreeMap<String, String>,
    docs: ToolDocs,
}

impl HttpGetFn {
    pub(crate) fn new(responses: BTreeMap<String, String>) -> Self {
        Self {
            responses,
            docs: ToolDocs {
                name: "http.get".to_string(),
                namespace: "http".to_string(),
                summary: "Fetch a deterministic allowlisted HTTP response".to_string(),
                description: Some(
                    "M1/P0 exposes http.get as a default-deny deterministic allowlist helper. It is not ambient network egress, not fetch(), and not a production egress policy; production egress hardening remains deferred."
                        .to_string(),
                ),
                signature: "http.get(url: string): Promise<string>".to_string(),
                args_schema: json!({
                    "type": "object",
                    "required": ["url"],
                    "additionalProperties": false,
                    "properties": {
                        "url": {
                            "type": "string",
                            "format": "uri",
                            "description": "URL must be present in the session's deterministic allowlist."
                        }
                    }
                }),
                result_schema: Some(json!({ "type": "string" })),
                examples: vec![ToolExample {
                    title: Some("Fetch allowlisted fixture".to_string()),
                    code: "const body = await http.get('https://local.test/ok');\ndisplay(body);"
                        .to_string(),
                    notes: Some(
                        "Non-allowlisted URLs fail closed with CapabilityDeniedError; this helper does not grant open network egress."
                            .to_string(),
                    ),
                }],
                errors: vec![
                    ToolErrorDoc {
                        name: "CapabilityDeniedError".to_string(),
                        when: "The URL is not in the session deterministic allowlist or http.get is not granted."
                            .to_string(),
                        retryable: false,
                    },
                    ToolErrorDoc {
                        name: "InvalidArgsError".to_string(),
                        when: "The url argument is missing or not a string.".to_string(),
                        retryable: false,
                    },
                ],
                grants: vec![GrantDoc {
                    kind: "network".to_string(),
                    description:
                        "Deterministic allowlisted HTTP fixture access only; no open egress.".to_string(),
                }],
                sensitive: true,
                approval: "none".to_string(),
                since: "M1".to_string(),
                stability: "experimental".to_string(),
            },
        }
    }
}

#[async_trait]
impl HostFn for HttpGetFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(
        &self,
        args: Value,
        _ctx: &InvocationCtx,
    ) -> std::result::Result<Value, HostError> {
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| HostError::InvalidArgs("http.get requires a string url".to_string()))?;
        self.responses
            .get(url)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| HostError::CapabilityDenied("http.get".to_string()))
    }
}
