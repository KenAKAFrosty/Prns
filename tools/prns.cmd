@echo off
python --version >nul 2>nul
if errorlevel 1 (py "%~dp0prns" %*) else (python "%~dp0prns" %*)
exit /b %errorlevel%
