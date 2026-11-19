#!/bin/sh
# usage: cat_timed.sh <file> <result-file>
# Wall-clock time for `cat <file>` inside whatever terminal runs this (comparable across terminals).
S=$(perl -MTime::HiRes=time -e 'printf "%.6f", time')
cat "$1"
E=$(perl -MTime::HiRes=time -e 'printf "%.6f", time')
echo "$S $E" > "$2"
sleep 1
