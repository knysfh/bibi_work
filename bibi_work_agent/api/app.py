from __future__ import annotations

from fastapi import FastAPI

from bibi_work_agent.api.internal_routes import router as internal_router


app = FastAPI(title="Bibi Work Agent Runtime", version="0.1.0")
app.include_router(internal_router)
