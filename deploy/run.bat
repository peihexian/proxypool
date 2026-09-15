@echo off
setlocal
cd /d "%~dp0"
if not exist "data" mkdir "data"
set "MYPROXY_DATA=%cd%\data"
if "%PORT%"=="" set "PORT=9090"
echo Starting ProxyPool on port %PORT% ...
echo Data directory: %MYPROXY_DATA%
myproxy.exe
