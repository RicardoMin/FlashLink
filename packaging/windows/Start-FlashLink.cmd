@echo off
setlocal EnableExtensions DisableDelayedExpansion
set "host=%~dp0FlashLink.exe"
if not exist "%host%" set "host=%~dp0flashlink-host.exe"
if not exist "%host%" (
    echo Cannot find FlashLink.exe. Extract the complete demo or build flashlink-host first.
    pause
    exit /b 1
)
if "%~1"=="" (
    start "" "%host%"
) else (
    start "" "%host%" --sunshine "%~1"
)
