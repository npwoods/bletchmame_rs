#!/bin/bash

set -euo pipefail
set -x

###################################################################################
# run_mame_integration_testing.sh - Runs integration testing against actual MAME  #
###################################################################################

# Sanity check
if [ -z "$BASH_SOURCE" ]; then
  echo "Null BASH_SOURCE"
  exit
fi

# Identify directories
SCRIPTS_DIR=$(dirname $BASH_SOURCE)
DEPS_DIR=$(dirname $BASH_SOURCE)/../deps

# Set up GNU Parallel's maximum line length (terrible, ya know)
mkdir -p ~/.parallel/tmp/sshlogin/$(uname -n)
echo 30000 > ~/.parallel/tmp/sshlogin/$(uname -n)/linelen

# Download `alienar` ROM
mkdir -p $DEPS_DIR/roms
curl -L "https://www.mamedev.org/roms/alienar/alienar.zip" > $DEPS_DIR/roms/alienar.zip

# Run it
parallel --joblog - $SCRIPTS_DIR/test_mame_interactions.sh ::: mame0230 mame0240 mame0260 mame0280
