@echo off
rem Wrapper omijajacy ExecutionPolicy - przekazuje argumenty do dev.ps1
rem   dev            sprawdza toolchain
rem   dev -Demo      uruchamia demo piora
rem   dev -Install   doinstalowuje braki
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" %*
