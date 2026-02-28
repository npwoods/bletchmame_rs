#!/bin/bash

set -euo pipefail

################################################################################################
# test_mame_interactions.sh - Use diagnostic features to exercise a particular version of MAME #
################################################################################################

# Determine the MAME version (e.g. - 'mame0227')
MAME_VERSION="$(echo -e "${1}" | tr -d '[:space:]')"
MAME_VERSION_NUMBER="$(echo -e "${MAME_VERSION}" | tr -d '[a-z]')"

# Determine the MAME executable name
if [ $MAME_VERSION_NUMBER -le 228 ]; then
    MAME_EXE=mame64.exe
else
    MAME_EXE=mame.exe
fi

# Abort if anything fails
set -e

# Download MAME binaries for Windows
mkdir -p deps
curl -f -L "https://github.com/mamedev/mame/releases/download/${MAME_VERSION}/${MAME_VERSION}b_64bit.exe" > deps/${MAME_VERSION}b_64bit.exe

# Extract the archive
7z -y x deps/${MAME_VERSION}b_64bit.exe -odeps/${MAME_VERSION}

# Ensure we can digest -listxml from this MAME
deps/${MAME_VERSION}/${MAME_EXE} -listxml | ./target/release/bletchmame.exe --process-listxml

# Finally clean up after ourselves
rm -rf deps/${MAME_VERSION}b_64bit.exe deps/${MAME_VERSION}/

# Report success
echo "Testing with $MAME_VERSION was successful"


