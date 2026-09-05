@echo off
rem DSH-View build wrapper: initialize MSVC x64 env, then run cargo commands in src-tauri
rem Usage: scripts\build_env.bat <command...>   e.g. scripts\build_env.bat cargo check
set "VCVARS="
for %%P in (
  "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
) do (
  if not defined VCVARS if exist %%P set "VCVARS=%%~P"
)
if not defined VCVARS (
  echo [error] vcvars64.bat not found. Install Visual Studio Build Tools (C++ workload) first.
  exit /b 1
)
call "%VCVARS%" >nul 2>&1
cd /d "%~dp0..\src-tauri"
%*
