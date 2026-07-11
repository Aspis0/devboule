"""Oracle LLM provider privacy and ZDR/GDPR compliance."""

ALLOWED_PROVIDERS = {"scaleway", "infomaniak", "mistral"}
LOCAL_PROVIDERS = {"omlx", "ollama"}


class OraclePrivacyGate:
    """Enforces privacy requirements for Oracle LLM providers."""

    def __init__(self):
        self.allowed_providers = ALLOWED_PROVIDERS
        self.local_providers = LOCAL_PROVIDERS

    def validate_provider(self, provider, api_key=""):
        """Validate that a provider is allowlisted."""
        provider = provider.strip().lower()
        if provider not in self.allowed_providers | self.local_providers:
            return {"allowed": False, "reason": f"Provider {provider} is not allowlisted."}
        is_remote = provider not in self.local_providers
        if is_remote and not api_key:
            return {"allowed": False, "reason": "Remote provider requires API key."}
        return {"allowed": True, "reason": ""}

    def is_zdr_compliant(self, provider):
        """Check if a provider supports zero data retention."""
        return provider.lower() in {"infomaniak", "mistral"}

    def is_gdpr_compliant(self, provider):
        """Check if a provider is GDPR compliant."""
        return provider.lower() in {"infomaniak", "scaleway"}

    def get_oracle_llm_config(self, provider, model, base_url=""):
        """Build Oracle LLM configuration."""
        validation = self.validate_provider(provider)
        if not validation["allowed"]:
            raise ValueError(validation["reason"])
        default_urls = {
            "scaleway": "https://api.scaleway.ai/v1/chat/completions",
            "infomaniak": "https://api.infomaniak.com/2/ai/108646/openai/v1/chat/completions",
            "mistral": "https://api.mistral.ai/v1/chat/completions",
        }
        return {"provider": provider, "model": model, "base_url": base_url or default_urls.get(provider, ""), "zdr": self.is_zdr_compliant(provider), "gdpr": self.is_gdpr_compliant(provider)}
