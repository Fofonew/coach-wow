@echo off
rem Build environment for Mentor (Rust + whisper.cpp Vulkan + Tauri).
rem Usage: build.cmd [cargo arguments]   e.g. build.cmd build --release
rem        build.cmd tauri                builds the installer (npx tauri build)
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\CMake\bin;C:\Program Files\LLVM\bin;C:\VulkanSDK\1.4.363.0\Bin;%PATH%"
set "LIBCLANG_PATH=C:\Program Files\LLVM\bin"
set "VULKAN_SDK=C:\VulkanSDK\1.4.363.0"
set "CARGO_TARGET_DIR=C:\cwt"
set "WHISPER_DONT_GENERATE_BINDINGS="
cd /d "%~dp0"
if /i "%1"=="tauri" (
    if not exist C:\cwt\release\sherpa-onnx-c-api.dll (
        echo Lance d'abord : build.cmd build --release
        exit /b 1
    )
    if not exist src-tauri\dlls mkdir src-tauri\dlls
    for %%F in (sherpa-onnx-c-api.dll onnxruntime.dll onnxruntime_providers_shared.dll) do copy /y "C:\cwt\release\%%F" src-tauri\dlls\ >nul
    for /d %%D in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Redist\MSVC\14.*") do (
        for %%F in (msvcp140.dll msvcp140_1.dll vcruntime140.dll vcruntime140_1.dll) do if exist "%%D\x64\Microsoft.VC143.CRT\%%F" copy /y "%%D\x64\Microsoft.VC143.CRT\%%F" src-tauri\dlls\ >nul
    )
    call npx tauri build
) else (
    cd src-tauri
    cargo %*
)
