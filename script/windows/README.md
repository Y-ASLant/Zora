# Inno Setup installer script

## What is `windows-installer.iss`?

On Windows, programs are conventionally installed using an installer, also known as an installation wizard.
The installer is a single executable that takes care of:
* Creating a directory to store the program's files
* Downloading assets
* Initializing registry entries
* Creating a desktop icon
* ... and more, depending on the application's needs.


`windows-installer.iss` is an **Inno Setup script**:
a configuration file for building a Zora installer.
The Inno Setup Compiler takes a script file and generates an installer executable.
This is roughly equivalent to the bundling process on MacOS.


## How to edit the installer

See the Inno Setup documentation: [Inno Setup Help](https://jrsoftware.org/ishelp/).
This script can be edited manually using any code editor.
The Inno Setup compiler turns this script into an installer `.exe`.


## Build a Zora installer

For a normal Windows release build, run this from the repository root:

```powershell
make build
```

This invokes `bundle.ps1`, builds the correct OSS executable, prepares bundled resources and then runs Inno Setup. To set the release version explicitly:

```powershell
make build RELEASE_TAG=vYYYY.MM.DD.1
```

The OSS installer is normally written to `script/windows/Output/ZoraSetup.exe`.

To reclaim Cargo build-cache space without deleting generated installers:

```powershell
make clean
```

`make clean` runs `cargo clean`; the next build recompiles dependencies.

## Manual Inno Setup troubleshooting

Only use this path when diagnosing the installer itself. First ensure the environment is ready:

* Download and install the [Inno Setup Compiler](https://jrsoftware.org/isdl.php).
* Run `make build` once so the executable and bundled resources are current.

### Option 1: Use the CLI
1. Add the Inno Setup Command-line Compiler executable to your shell path.
By default, it is located at `C:\Program Files (x86)\Inno Setup 6\ISCC.exe`.
2. Compile the installer. Supply the build-wrapper values explicitly; `Arch` and `OutputName` have no fallback in the script:
```shell
iscc .\script\windows\windows-installer.iss /DArch=x64 /DOutputName=ZoraSetup /DReleaseChannel=oss /DMyAppName=Zora /DMyAppExeName=zora.exe /DTargetProfileDir=target\x86_64-pc-windows-msvc\rlto /DMyAppVersion=vYYYY.MM.DD.1 /DAppUserModelId=dev.warp.zora /DInnoAppId=dev.warp.zora
```
3. Run the generated executable:
```shell
.\script\windows\Output\ZoraSetup.exe
```

The build wrapper normally passes the Inno Setup preprocessor definitions. For manual debugging, keep these values consistent with the binary you already built:
* `ReleaseChannel` (`oss` for Zora OSS releases)
* `MyAppName` (`Zora` for OSS)
* `MyAppVersion`
* `MyAppExeName`
* `TargetProfileDir`
* `Arch`
* `OutputName`
* `AppUserModelId`
* `InnoAppId`

### Option 2: Use the GUI
1. Open the Inno Setup application and select this script.
2. Click the "compile" button. This will generate an installer executable in a directory called `Output` at the same level as this script.
3. To run the installer, click the "run" button in Inno Setup.


## Using icons

Windows has its own icon file format that bundles together multiple icon sizes.
App icons are located in `app/channels/<channel_name>/icon/padded` (内含 ~10% safe-area, macOS dock 与 Linux/Windows 共用).
The `.ico` files are generated using imagemagick:

```shell
convert 16x16.png 32x32.png 48x48.png 64x64.png 256x256.png icon.ico
```

Note that sizes above 256x256 are not supported.
See the [Inno Setup docs](https://jrsoftware.org/ishelp/index.php?topic=setup_setupiconfile).
