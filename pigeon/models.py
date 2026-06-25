from pydantic import BaseModel
from typing import Optional

class SendRequest(BaseModel):
    sender_id: str
    receiver_id: str
    project_id: str
    priority: int = 50
    payload: dict

class SendResponse(BaseModel):
    ticket_no: int
    status: str
    delivery_mode: str
    receiver_status: str
    eta_seconds: Optional[int] = None

class PollResponse(BaseModel):
    ticket_no: Optional[int] = None
    payload: Optional[dict] = None

class DoneRequest(BaseModel):
    ticket_no: int
    result: dict

class Agent(BaseModel):
    agent_id: str
    agent_type: str = "local"
    status: str = "unloaded"
    last_seen: Optional[int] = None

class Task(BaseModel):
    ticket_no: int
    sender_id: str
    receiver_id: str
    project_id: str
    priority: int
    status: str
    delivery_mode: str
    payload: dict
    result: Optional[dict] = None
    error_msg: Optional[str] = None
    reply_to_ticket: Optional[int] = None
    created_at: int
    claimed_at: Optional[int] = None
    done_at: Optional[int] = None
