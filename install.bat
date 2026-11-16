@echo off
cd /d "%~dp0"
if not exist .venv (
    py -3.12 -m venv .venv || python -m venv .venv
)
.venv\Scripts\python -m pip install --upgrade pip
.venv\Scripts\python -m pip install -r requirements.txt
echo.
echo Installation terminee. Lance start_coach.bat
pause
