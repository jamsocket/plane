#!/bin/sh
set -euf

usage() {
	echo usage: "$(basename "$0")" '[-[lmxrcgb] --linux --macos --x11 --guac --clean --build]' 1>&2
	exit 1
}

long_opts() {
	case $1 in
	linux) [ ! "$OS" ] && linux=1 OS=1 || exit 1;;
	macos) [ ! "$OS" ] && macos=1 OS=1 || exit 1;;
	x11) [ ! "$BROWSER_OPT" ] && x11=1 BROWSER_OPT=1 || exit 1;;
	guac) [ ! "$BROWSER_OPT" ] && guac=1 BROWSER_OPT=1 || exit 1;;	
	clean) [ ! "$CLEAN_OPT" ] && CLEAN_OPT=1 || exit 1;;
	build) [ ! "$BUILD_OPT" ] && BUILD_OPT=1 || exit 1;;
	?) usage "" && exit 1;;
 	esac
}

clean() {
	echo "not implemented!"
	exit 1
}

if [ ! -d "$PWD/compose" ]
then
	echo "This script must be run from the plane/sample-config directory"
	exit 1
fi

COMPOSE_FILE_DIR="$PWD/compose"
PLANE_COMPOSE="$COMPOSE_FILE_DIR/plane.yml"
FIREFOX_X11_COMPOSE="$COMPOSE_FILE_DIR/firefox-x11.yml"
DOCKER_COMPOSE="docker compose"


linux='' macos='' x11='' guac='' OS='' BUILD_OPT='' BROWSER_OPT='' CLEAN_OPT='' BROWSER_CMD=""
while getopts "lmxrcgb-:" arg
do
	case $arg in 
	l) [ ! "$OS" ] && linux=1 OS=1 BROWSER_CMD="xdg-open" || exit 1;;
	m) [ ! "$OS" ] && macos=1 OS=1 BROWSER_CMD="open" || exit 1;;
	x) [ ! "$BROWSER_OPT" ] && x11=1 BROWSER_OPT=1 || exit 1;;
	g) [ ! "$BROWSER_OPT" ] && guac=1 BROWSER_OPT=1 || exit 1;;
	c) [ ! "$CLEAN_OPT" ] && CLEAN_OPT=1 || exit 1;;
	b) [ ! "$BUILD_OPT" ] && BUILD_OPT=1 || exit 1;;
	-) long_opts "$OPTARG"; exit $?;;
	?) usage "" && exit 1;;
	esac
done

FLAGS="--remove-orphans --pull always --no-build"
if [ $BUILD_OPT ]; then
	FLAGS="--remove-orphans --force-recreate --build"
fi

if [ $CLEAN_OPT ]; then
	clean ""
	exit $?
fi



if [ $linux ] && [ $x11 ]
then
	$DOCKER_COMPOSE -f "$PLANE_COMPOSE" -f "$FIREFOX_X11_COMPOSE" up "$FLAGS"
	exit $?
fi

if { [ $linux ] || [ $macos ]; } && [ $guac ]
then
	$DOCKER_COMPOSE -f "$PLANE_COMPOSE" up -d "$FLAGS" &&\
	$BROWSER_CMD "http://localhost:3000" &&\
	$DOCKER_COMPOSE -f "$PLANE_COMPOSE" up -d
	exit $?
fi

if [ $macos ] && [ $x11 ]
then
	echo "unimplemented!"
	exit 1
fi

usage ""
