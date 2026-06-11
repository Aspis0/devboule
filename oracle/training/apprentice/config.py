from pathlib import Path

BASE = Path(__file__).resolve().parent
DATA = BASE / "data"
LOGS = DATA / "logs"
ADP = BASE / "adapters"

MODEL_PATH = BASE.parent / "day0" / "qwen3-27b-day0-mlx"
EMBED_MODEL = "Qwen/Qwen3-Embedding-0.6B"
SIM_THRESHOLD = 0.85
CLUSTER_TRIGGER = 30
DECAY_DAYS = 14
VICTORY_STREAK = 3
TRAIN_ITERS = 200
LORA_RANK = 8
BATCH_SIZE = 4
GOLD_SAMPLE = 50
REG_TOLERANCE = 0.05
SANDBOX_TIMEOUT = 10
SANDBOX_MEMORY_MB = 1024

# Replay replay budget so a single cluster never grows unbounded.
REPLAY_CAP = 200
