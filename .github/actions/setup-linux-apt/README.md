# Linux package downloads

This action prepares hosted Linux runners before APT package installation. It
replaces the Azure Ubuntu mirror in the runner's mirror list with Ubuntu's
official HTTPS archive and limits download attempts and connection timeouts.
Other package sources and APT package verification remain unchanged.

Android service, target-ISA emulator, and Swift setup use this shared preparation
before their existing dependency installation steps.
