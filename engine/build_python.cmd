@echo off
rem Build the vgcsim Python module and install it into the ACTIVE virtual environment (Windows).
rem Uses pgo\windows.profdata when present (bash tools/pgo.sh makes it; set PGO=0 for a plain build).
if "%VIRTUAL_ENV%"=="" ( echo activate a virtual environment first & exit /b 1 )
set "VGC=%~dp0"
if not "%PGO%"=="0" if exist "%VGC%pgo\windows.profdata" set "RUSTFLAGS=%RUSTFLAGS% -Cprofile-use=%VGC%pgo\windows.profdata -Cllvm-args=-no-pgo-warn-mismatch"
pushd "%VGC%py"
pip install -q "maturin>=1.5"
maturin develop --release
popd
