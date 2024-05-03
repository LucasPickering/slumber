#!/bin/sh
# Regenerate the demo GIF from the VHS tape
# https://github.com/charmbracelet/vhs

rm -rf data/ # Delete temp data so the GIF is consistent
vhs static/demo.tape
