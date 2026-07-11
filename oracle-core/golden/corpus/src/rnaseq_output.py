"""RNA-seq output rendering and artifact download pipeline."""

class OutputRenderer:
    """Manages RNA-seq output render lifecycle."""

    def __init__(self, job_id, artifact_url, manifest_url):
        self.job_id = job_id
        self.artifact_url = artifact_url
        self.manifest_url = manifest_url
        self.output_renders = []

    def request_output_render_record(self, payload):
        """Request an output render record."""
        if self._get_status() != "done":
            return {"error": "outputs_not_ready"}
        existing = self._find_ready_render()
        if existing:
            return existing
        return self._create_output_render_record(payload)

    def sync_output_render_status(self, callback_data):
        """Sync the output render status from the callback."""
        status = callback_data.get("status", "")
        if status == "ready":
            self._store_render(callback_data)
        return self._normalize_status_payload(callback_data)

    def download_rendered_artifact(self, artifact_id):
        """Download a rendered artifact after verifying it is registered."""
        render = self._find_render_by_id(artifact_id)
        if not render or not render.get("registered"):
            raise ValueError("Artifact not registered")
        return self._fetch_artifact(render["artifact_url"])

    def sanitize_output_renders(self, renders):
        """Sanitize render records for client consumption."""
        return [self._sanitize_render_record(r) for r in renders]

    def _get_status(self):
        return "done"

    def _find_ready_render(self):
        for r in self.output_renders:
            if r.get("status") == "ready":
                return r
        return None

    def _create_output_render_record(self, payload):
        return {"status": "created", "render_id": "new_render"}

    def _store_render(self, data):
        self.output_renders.append(data)

    def _normalize_status_payload(self, data):
        return {"status": data.get("status", "unknown")}

    def _find_render_by_id(self, artifact_id):
        for r in self.output_renders:
            if r.get("id") == artifact_id:
                return r
        return None

    def _fetch_artifact(self, url):
        return b"artifact content"

    def _sanitize_render_record(self, render):
        return {"id": render.get("id"), "status": render.get("status"), "content_disposition": "attachment"}


class RunnerStatusNormalizer:
    """Normalizes runner status payloads for the RNA-seq pipeline."""

    @staticmethod
    def normalize_runner_status_payload(payload):
        status = payload.get("status", "unknown")
        provider_message = payload.get("providerMessage", "")
        if status == "done":
            provider_message = "Results ready"
        return {"status": status, "providerMessage": provider_message, "output_renders": payload.get("output_renders", [])}
