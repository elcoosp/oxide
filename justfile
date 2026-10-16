# Run a script under watchexec, restarting on save. `--stop-signal=SIGKILL`
# + `--wrap-process=session` guarantee the ENTIRE process tree of the old run
# (cargo, oxide-cli build/start, backgrounded servers) is dead before the new
# run starts. Without this, bash does not forward SIGTERM to its children, the
# backgrounded `oxide-cli start` orphans and keeps running, and two runs
# overlap — colliding on shared roots, the redb CAS lock, and the git index.
wr:
    watchexec -w ./wr.sh --clear -r --wrap-process=session --stop-signal=SIGKILL --stop-timeout=200ms "./wr.sh"
wr1:
    watchexec -w ./wr1.sh --clear -r --wrap-process=session --stop-signal=SIGKILL --stop-timeout=200ms "./wr1.sh"
