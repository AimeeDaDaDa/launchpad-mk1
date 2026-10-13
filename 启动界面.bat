@echo off
rem 启动界面.bat — 双击打开 Launchpad 声音板控制台（本地网页）
cd /d %~dp0
set PY=C:\Users\Administrator\.workbuddy\binaries\python\envs\default\Scripts\python.exe
if not exist "%PY%" (
  where python >nul 2>nul && set PY=python
)
"%PY%" gui\gui_server.py 8787
pause
