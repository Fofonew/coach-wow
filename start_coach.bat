@echo off
cd /d "%~dp0"
title Coach WoW
if not exist .venv\Scripts\python.exe (
    echo Lance d'abord install.bat
    pause
    exit /b 1
)
.venv\Scripts\python coach.py %*
pause
