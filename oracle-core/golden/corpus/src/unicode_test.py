# -*- coding: utf-8 -*-
# Test file for Unicode handling in the chunking pipeline.

FRENCH_TEXT = "Les agents de recherche utilisent l'API Oracle pour interroger le code."
SPANISH_TEXT = "Los agentes reclaman tareas y actualizan el estado del proyecto."
GERMAN_TEXT = "Die Konfiguration der Scaleway-Instanzen erfolgt ueber die API."
ITALIAN_TEXT = "Il codice dell'ingestione gestisce il chunking semantico."

EMOJI_CONFIG = {
    "status": "completed",
    "pending": "in progress",
    "failed": "error",
    "warning": "check needed",
    "rocket": "deployment ready",
    "brain": "AI model loaded",
}

CJK_TITLE = "Oracle code index pipeline"
CJK_DESCRIPTION = "This module handles text chunking and semantic indexing of source files."
CJK_FUNCTION_NAMES = {
    "chunk": "chunking",
    "embed": "embedding",
    "index": "indexing",
    "query": "querying",
    "search": "searching",
}

def generate_chunk_id(file_id: str, index: int) -> str:
    return f"{file_id}#chunk-{index:04d}"
