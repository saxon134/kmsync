@echo off
setlocal enabledelayedexpansion

set "ARGS="
set "SKIP_NEXT=0"

:parse
if "%~1"=="" goto run
if "!SKIP_NEXT!"=="1" (
  set "SKIP_NEXT=0"
  shift
  goto parse
)

set "ARG=%~1"
if "!ARG!"=="-arch" (
  set "SKIP_NEXT=1"
  shift
  goto parse
)
if "!ARG!"=="-gfull" (
  shift
  goto parse
)
if not "!ARG:-mmacosx-version-min=!"=="!ARG!" (
  shift
  goto parse
)

set ARGS=!ARGS! "%~1"
shift
goto parse

:run
gcc %ARGS%
exit /b %ERRORLEVEL%
