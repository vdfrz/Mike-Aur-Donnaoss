"""GLiNER NER sidecar for Mike's PII anonymizer.

Exposes a single POST /detect endpoint that accepts text + labels and
returns detected entity spans.  The Rust backend calls this for name /
org / address detection that regex can't cover.

Run:
    pip install -r requirements.txt
    python app.py                       # listens on :4010

ENV overrides:
    GLINER_MODEL   model id   (default: urchade/gliner_multi_pii-v1)
    GLINER_PORT    listen port (default: 4010)
"""

import os
import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI
from pydantic import BaseModel
from gliner import GLiNER

logging.basicConfig(level=logging.INFO)
log = logging.getLogger("gliner-sidecar")

MODEL_ID = os.environ.get("GLINER_MODEL", "urchade/gliner_multi_pii-v1")

# ── model singleton ───────────────────────────────────────────────────

model: GLiNER | None = None


@asynccontextmanager
async def lifespan(_app: FastAPI):
    global model
    log.info(f"Loading GLiNER model: {MODEL_ID}")
    model = GLiNER.from_pretrained(MODEL_ID)
    log.info("Model loaded ✓")
    yield
    model = None


app = FastAPI(title="GLiNER NER sidecar", lifespan=lifespan)

# ── types ─────────────────────────────────────────────────────────────


class DetectRequest(BaseModel):
    text: str
    labels: list[str]
    threshold: float = 0.35


class Entity(BaseModel):
    text: str
    start: int
    end: int
    label: str
    score: float


# ── endpoint ──────────────────────────────────────────────────────────


@app.post("/detect", response_model=list[Entity])
def detect(req: DetectRequest):
    """Run GLiNER on the input text and return entity spans."""
    entities = model.predict_entities(
        req.text,
        req.labels,
        threshold=req.threshold,
    )
    return [
        Entity(
            text=e["text"],
            start=e["start"],
            end=e["end"],
            label=e["label"],
            score=round(e["score"], 4),
        )
        for e in entities
    ]


@app.get("/health")
def health():
    return {"status": "ok", "model": MODEL_ID}


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("GLINER_PORT", "4010"))
    uvicorn.run(app, host="0.0.0.0", port=port)
