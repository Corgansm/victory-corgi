@echo off
rem Starts the prologue pilot on the victory.exe that is running (the harness launched it with
rem VICTORY_INPUT_FILE=%USERPROFILE%\victory-mcp\input.txt) and returns at once; the pilot's own
rem lines go to runs\prologue-pilot.log. Arguments are passed on (e.g. --route enter).
setlocal
set "WORK=%USERPROFILE%\victory-mcp"
set "GAME=D:\GTA 6 Xbox Build\GTA6.Xbox.v1.0.2.1-CYBERLEEK\game_scarlett_master.exe"
if not "%VICTORY_GAME_EXE%"=="" set "GAME=%VICTORY_GAME_EXE%"
start "VIctory prologue pilot" /min cmd /c ""%WORK%\.venv\Scripts\python.exe" "%WORK%\prologue.py" --game-exe "%GAME%" --work "%WORK%" --attach --restarts 0 %* > "%WORK%\runs\prologue-pilot.log" 2>&1"
