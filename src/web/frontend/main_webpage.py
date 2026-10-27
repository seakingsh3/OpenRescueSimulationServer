from fastapi import Request
from fastapi.responses import HTMLResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates

import os

def setup_web_routes(app):
    base_path = os.path.dirname(__file__)
    static_path = os.path.join(base_path, "static")
    template_path = os.path.join(base_path, "templates")

    app.mount("/static", StaticFiles(directory=static_path), name="static")
    templates = Jinja2Templates(directory=template_path)

    @app.get("/", response_class=HTMLResponse)
    async def index_page(request: Request):
        return templates.TemplateResponse("index.html", {"request": request})

    @app.get("/record", response_class=HTMLResponse)
    async def record_page(request: Request):
        return templates.TemplateResponse("record.html", {"request": request})

    @app.get("/options", response_class=HTMLResponse)
    async def options_page(request: Request):
        return templates.TemplateResponse("options.html", {"request": request})

    @app.get("/viewer", response_class=HTMLResponse)
    async def options_page(request: Request):
        return templates.TemplateResponse("viewer.html", {"request": request})

    @app.get("/simulate", response_class=HTMLResponse)
    async def options_page(request: Request):
        return templates.TemplateResponse("simulate.html", {"request": request})

