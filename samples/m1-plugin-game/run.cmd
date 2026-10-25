@echo off
cd /d "%~dp0"
charon-m1-sample.exe %*
if errorlevel 1 pause
