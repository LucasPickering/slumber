#!/bin/sh
# Regenerate the demo GIF from the VHS tape
# https://github.com/charmbracelet/vhs

case $1 in
    "--check")
        latest_commit=$(git rev-parse HEAD)
        latest_gif_commit=$(git log -n 1 --pretty=format:%H -- static/demo.gif)
        if [ $latest_commit = $latest_gif_commit ]; then
            echo "Good to go!"
        else
            echo "Demo gif is out of date"
            echo "Run './demo.sh' to regenerate"
            exit 1
        fi
        ;;
    "")
        rm -rf data/ # Delete temp data so the GIF is consistent
        cargo build # Make sure the recording doesn't capture the build process
        vhs static/demo.tape
        echo "Don't forget to look at the gif before pushing!"
        ;;
    *)
        echo "Invalid args: $@"
        exit 1
esac
