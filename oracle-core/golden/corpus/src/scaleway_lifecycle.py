"""Scaleway instance lifecycle management for RNA-seq compute."""

class ScalewayInstanceManager:
    """Manages Scaleway compute instance lifecycle."""

    def __init__(self, project_id, zone="fr-par-1"):
        self.project_id = project_id
        self.zone = zone
        self.active_instance_key = None
        self.instances = {}

    def create_instance(self, commercial_type):
        instance_id = f"scw-{commercial_type}-{len(self.instances)}"
        self.instances[instance_id] = {"id": instance_id, "commercial_type": commercial_type, "status": "running"}
        self.active_instance_key = instance_id
        return self.instances[instance_id]

    def cleanup_scaleway_instance_after_terminal(self, job_id):
        """Clean up a Scaleway instance after terminal status."""
        instance_id = self._find_instance_for_job(job_id)
        if not instance_id:
            return {"status": "no_instance"}
        self.terminate_scaleway_instance(instance_id)
        self.release_scaleway_instance_slot()
        return {"status": "cleaned", "instance_id": instance_id}

    def terminate_scaleway_instance(self, instance_id):
        """Terminate a Scaleway instance."""
        instance = self.instances.get(instance_id)
        if not instance:
            return {"error": "not_found"}
        instance["status"] = "terminated"
        return {"status": "terminated", "with_volumes": "all"}

    def release_scaleway_instance_slot(self):
        """Release the active instance slot."""
        self.active_instance_key = None

    def bare_delete(self, instance_id):
        """Perform a bare delete."""
        if instance_id in self.instances:
            del self.instances[instance_id]
            return {"status": "deleted"}
        return {"error": "not_found"}

    def _find_instance_for_job(self, job_id):
        for iid, inst in self.instances.items():
            if inst["status"] == "running":
                return iid
        return None
