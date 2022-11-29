#!/bin/sh
set -euf

usage() {
	echo usage: "$(basename "$0")" '[-[lmxrocgb] --linux --macos --x11 --cluster --guac --clean --build]' 1>&2
	exit 1
}

long_opts() {
	case $1 in
	linux) [ ! "$OS" ] && linux=1 OS=1 || exit 1;;
	macos) [ ! "$OS" ] && macos=1 OS=1 || exit 1;;
	x11) [ ! "$BROWSER_OPT" ] && x11=1 BROWSER_OPT=1 || exit 1;;
	guac) [ ! "$BROWSER_OPT" ] && BROWSER_OPT=1 || exit 1;;	
	clean) CLEAN_OPT=1;;
	cluster) NATS_CLUSTER=1;;
	build) [ ! "$BUILD_OPT" ] && BUILD_OPT=1 || exit 1;;
	debug) debug=1;;
	?) usage "" && exit 1;;
 	esac
}

clean() {
	# kill orphaned plane backends
	# note: this is required, otherwise the plane network can't be dismantled
	orphaned_containers=$(docker container ls -q -f name=^plane*) 
	docker kill "$orphaned_containers" || echo "no orphaned backends!"
	
	# shellcheck disable=2086 # I actually want splitting here
	$DOCKER_COMPOSE -f $PLANE_COMPOSE -f $FIREFOX_X11_COMPOSE -f $NATS_CLUSTER_COMPOSE down --remove-orphans --rmi all
	exit 0
}

build_compose_up() {
	#arg array consists of compose files
	#nats flags set in NATS_FLAGS env var
	#compose flags set in FLAGS env var
	COMPOSE_FILES=''
	for compose_file in "$@"
	do
		COMPOSE_FILES="$COMPOSE_FILES -f $compose_file"
	done
	
	# shellcheck disable=2086 # I actually want splitting here
	NATS_FLAGS=$NATS_FLAGS $DOCKER_COMPOSE $COMPOSE_FILES up $FLAGS
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
NATS_CLUSTER_COMPOSE="$COMPOSE_FILE_DIR/nats-cluster.yml"


linux='' macos='' x11='' debug='' OS='' BUILD_OPT='' BROWSER_OPT='' CLEAN_OPT=''
NATS_CLUSTER=''
while getopts "lmxrocdgb-:" arg
do
	case $arg in 
	l) [ ! "$OS" ] && linux=1 OS=1 || exit 1;;
	m) [ ! "$OS" ] && macos=1 OS=1 || exit 1;;
	x) [ ! "$BROWSER_OPT" ] && x11=1 BROWSER_OPT=1 || exit 1;;
	g) [ ! "$BROWSER_OPT" ] && BROWSER_OPT=1 || exit 1;;
	c) CLEAN_OPT=1;;
	b) [ ! "$BUILD_OPT" ] && BUILD_OPT=1 || exit 1;;
	o) NATS_CLUSTER=1;;
	d) debug=1;;
	-) long_opts "$OPTARG" || exit 1;;
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

set -- #unsets positional param array in order to build up compose files
set "$PLANE_COMPOSE"

if [ $linux ] && [ $x11 ]
then
	set -- "$@" "$FIREFOX_X11_COMPOSE"
fi

if [ $NATS_CLUSTER ]
then
	set -- "$@" "$NATS_CLUSTER_COMPOSE"
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

build_compose_up "$@" || usage ""
