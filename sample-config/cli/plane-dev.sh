#!/bin/sh
set -euf

usage() {
	echo usage: "$(basename "$0")" '[-[lmxg]]' 1>&2
	exit 1
}

long_opts() {
	case $1 in
	linux) [ ! "$OS" ] && linux=1 OS=1 || exit 1;;
	macos) [ ! "$OS" ] && macos=1 OS=1 || exit 1;;
	x11) [ ! "$BROWSER_OPT" ] && x11=1 || exit 1;;
	guac) [ ! "$BROWSER_OPT" ] && guac=1 || exit 1;;	
	?) usage ""
	esac
}

if [ ! -d "$PWD/compose" ]
then
	echo "This script must be run from the plane/sample-config directory"
	exit 1
fi

COMPOSE_FILE_DIR="$PWD/compose"
DOCKER_COMPOSE="docker compose"


linux='' macos='' x11='' guac='' OS='' BROWSER_OPT=''
while getopts "lmxg-:" arg
do
	case $arg in 
	l) [ ! "$OS" ] && linux=1 OS=1 || exit 1;;
	m) [ ! "$OS" ] && macos=1 OS=1 || exit 1;;
	x) [ ! "$BROWSER_OPT" ] && x11=1 || exit 1;;
	g) [ ! "$BROWSER_OPT" ] && guac=1 || exit 1;;
	-) long_opts "$OPTARG" || exit 1;;
	?) usage ""
	esac
done

if [ $linux ] && [ $x11 ]
then
	$DOCKER_COMPOSE -f "$COMPOSE_FILE_DIR/plane.yml" -f "$COMPOSE_FILE_DIR/firefox-x11.yml" up --remove-orphans --force-recreate --build
	exit 0
fi

if { [ $linux ] || [ $macos ]; } && [ $guac ]
then
	$DOCKER_COMPOSE -f "$COMPOSE_FILE_DIR/plane.yml" up --remove-orphans --force-recreate --build & xdg-open "http://localhost:3000"
	exit 0
fi

if [ $macos ] && [ $x11 ]
then
	echo "unimplemented!"
	exit 1
fi

usage ""
