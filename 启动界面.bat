@echo off
rem 启动界面.bat — 双击打开 Launchpad 声音板控制台（本地网页）
cd /d %~dp0
set PY=C:\Users\Administrator\.workbuddy\binaries\python\envs\default\Scripts\python.exe
if not exist "%PY%" (
  where python >nul 2>nul && set PY=python
)
"%PY%" gui\gui_server.py 8787
rem 正常退出就直接关窗口；只有出错时才停住让你看报错
rem（原来是无条件 pause，在后台/自动化里会一直挂着）
if errorlevel 1 pause
