#!/bin/sh

# Generate favicon.ico using Imagemagick

convert -background transparent slumber.png -define icon:auto-resize=16,24,32,48,64 favicon.ico
