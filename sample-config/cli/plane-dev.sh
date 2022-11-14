#!/bin/sh
set -euf

usage() {
	echo usage: "$(basename "$0")" '[-[lmxrcgb] --linux --macos --x11 --guac --clean --build]' 1>&2
	exit 1
}

wait_until_guac() {
	until [ "$( curl -s http://localhost:3000/ )" ]; do
	    sleep 0.1;
	done;
}

long_opts() {
	case $1 in
	linux) [ ! "$OS" ] && linux=1 OS=1 || exit 1;;
	macos) [ ! "$OS" ] && macos=1 OS=1 || exit 1;;
	x11) [ ! "$BROWSER_OPT" ] && x11=1 BROWSER_OPT=1 || exit 1;;
	guac) [ ! "$BROWSER_OPT" ] && guac=1 BROWSER_OPT=1 || exit 1;;	
	clean) [ ! "$CLEAN_OPT" ] && CLEAN_OPT=1 || exit 1;;
	build) [ ! "$BUILD_OPT" ] && BUILD_OPT=1 || exit 1;;
	debug) debug=1;;
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
NATS_CLUSTER_COMPOSE="$COMPOSE_FILE_DIR/nats-cluster.yml"
NATS_SINGLE_COMPOSE="$COMPOSE_FILE_DIR/nats.yml"
NATS_COMPOSE="$NATS_SINGLE_COMPOSE"
DOCKER_COMPOSE="docker compose"


linux='' macos='' x11='' guac='' debug='' OS='' BUILD_OPT='' BROWSER_OPT='' CLEAN_OPT='' BROWSER_CMD="" nats_cluster=''
while getopts "lmxrcdgnb-:" arg
do
	case $arg in 
	l) [ ! "$OS" ] && linux=1 OS=1 BROWSER_CMD="xdg-open" || exit 1;;
	m) [ ! "$OS" ] && macos=1 OS=1 BROWSER_CMD="open" || exit 1;;
	x) [ ! "$BROWSER_OPT" ] && x11=1 BROWSER_OPT=1 || exit 1;;
	g) [ ! "$BROWSER_OPT" ] && guac=1 BROWSER_OPT=1 || exit 1;;
	c) [ ! "$CLEAN_OPT" ] && CLEAN_OPT=1 || exit 1;;
	b) [ ! "$BUILD_OPT" ] && BUILD_OPT=1 || exit 1;;
	n) nats_cluster=1 ;;
	d) debug=1;;
	-) long_opts "$OPTARG"; exit $?;;
	?) usage "" && exit 1;;
	esac
done

FLAGS="--remove-orphans --pull always"
if [ $BUILD_OPT ]; then
	FLAGS="--remove-orphans --force-recreate --build"
fi

if [ $CLEAN_OPT ]; then
	clean ""
	exit $?
fi

NATS_FLAGS=""
if [ $debug ]; then
	NATS_FLAGS="-DV"
fi

if [ $nats_cluster ]; then
	NATS_COMPOSE=$NATS_CLUSTER_COMPOSE
fi

if [ $linux ] && [ $x11 ]
then
	# shellcheck disable=2086 # I actually want splitting here
	NATS_FLAGS=$NATS_FLAGS $DOCKER_COMPOSE -f "$PLANE_COMPOSE" -f "$FIREFOX_X11_COMPOSE" -f "$NATS_COMPOSE" up $FLAGS
	exit $?
fi

if { [ $linux ] || [ $macos ]; } && [ $guac ]
then
	# shellcheck disable=2086 # I actually want splitting here
	NATS_FLAGS=$NATS_FLAGS $DOCKER_COMPOSE -f "$PLANE_COMPOSE" -f "$NATS_COMPOSE" up $FLAGS &\
	{ wait_until_guac "" && $BROWSER_CMD "http://localhost:3000" ;}
	exit $?
fi

if [ $macos ] && [ $x11 ]
then
	# /opt/X11/bin/Xquartz :1 -listen tcp &
	# DISPLAY=:1 /opt/X11/bin/quartz-wm &
	# DISPLAY=:1 xhost +localhost || { echo "please install xquartz and enable network access to it!" 1>&2 ; exit 1 ;}
	# ip=$(ifconfig en0 | grep inet | awk '$1=="inet" {print $2}')
	# # shellcheck disable=2086 # I actually want splitting here
	# XAUTHORITY="$HOME/.Xauthority" DISPLAY="$ip:1" $DOCKER_COMPOSE -f "$PLANE_COMPOSE" -f "$FIREFOX_X11_COMPOSE" up $FLAGS
	# exit $?
	echo "not implemented!" 1>&2
	exit 1
fi

usage ""
