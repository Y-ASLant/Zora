.PHONY: build clean version

CHANNEL ?= oss
RELEASE_TAG ?=
PACKAGES ?= appimage
ARCH ?=

clean:
	@echo Cleaning Cargo build artifacts...
	@cargo clean

ifeq ($(strip $(RELEASE_TAG)),)
ifeq ($(OS),Windows_NT)
RELEASE_TAG := v$(shell powershell.exe -NoProfile -Command "(Get-Date).ToString('yyyy.MM.dd')").1
else
RELEASE_TAG := v$(shell date +%Y.%m.%d).1
endif
endif

ifeq ($(OS),Windows_NT)

WINDOWS_BUILD_ARCH := $(if $(strip $(ARCH)),$(ARCH),$(if $(filter AMD64,$(PROCESSOR_ARCHITECTURE) $(PROCESSOR_ARCHITEW6432)),x64,$(if $(filter ARM64,$(PROCESSOR_ARCHITECTURE) $(PROCESSOR_ARCHITEW6432)),arm64,)))

version:
	@echo $(RELEASE_TAG)

build:
	@echo Building Zap $(RELEASE_TAG) for Windows...
	@powershell.exe -NoProfile -ExecutionPolicy Bypass -File "script/windows/bundle.ps1" -CHANNEL "$(CHANNEL)" -RELEASE_TAG "$(RELEASE_TAG)" $(if $(strip $(WINDOWS_BUILD_ARCH)),-ARCH "$(WINDOWS_BUILD_ARCH)")

else

UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Darwin)

version:
	@printf '%s\n' "$(RELEASE_TAG)"

build:
	@echo "Building Zap $(RELEASE_TAG) for macOS..."
	@./script/bundle --channel "$(CHANNEL)" --release-tag "$(RELEASE_TAG)" --nosign --nouniversal

else ifeq ($(UNAME_S),Linux)

version:
	@printf '%s\n' "$(RELEASE_TAG)"

build:
	@echo "Building Zap $(RELEASE_TAG) for Linux..."
	@./script/bundle --channel "$(CHANNEL)" --release-tag "$(RELEASE_TAG)" --packages "$(PACKAGES)"

else

$(error Unsupported platform: $(UNAME_S))

endif
endif
