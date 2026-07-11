"""Agent project workflow - MCP tools for task management."""


def read_project(project_id):
    """Read the current state of a project."""
    return {"id": project_id, "status": "active", "tasks": [], "current_task": None}


def project_list():
    """List all available projects."""
    return [{"id": "proj-001", "name": "Oracle Rust Port", "status": "active"}]


def project_claim_task(project_id, task_id, agent_id):
    """Claim a task for an agent."""
    return {"project_id": project_id, "task_id": task_id, "agent_id": agent_id, "status": "claimed"}


def project_update_status(project_id, task_id, status, agent_id):
    """Update the status of a claimed task."""
    valid_statuses = ["in_progress", "blocked", "review", "done", "failed"]
    if status not in valid_statuses:
        return {"error": f"Invalid status: {status}"}
    return {"project_id": project_id, "task_id": task_id, "status": status, "agent_id": agent_id}


def oracle_ask(query):
    """Ask the Oracle for architecture context."""
    return {"query": query, "answer": "", "citations": []}


def oracle_context(query, limit=5):
    """Get context chunks from the Oracle."""
    return []


class MCPToolRegistry:
    """Registry of MCP tools available to terminal agents."""

    TOOLS = {
        "project_list": project_list,
        "project_claim_task": project_claim_task,
        "project_update_status": project_update_status,
        "oracle_ask": oracle_ask,
        "oracle_context": oracle_context,
    }

    def call(self, tool_name, **kwargs):
        """Call a registered MCP tool."""
        tool = self.TOOLS.get(tool_name)
        if not tool:
            return {"error": f"Unknown tool: {tool_name}"}
        return tool(**kwargs)
