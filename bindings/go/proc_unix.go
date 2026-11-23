//go:build unix && !linux

package fakes3

import (
	"io"
	"os/exec"
	"strconv"
)

func prepare(*exec.Cmd) {}

// guard is a watchdog shell that holds the read end of a pipe whose write end only this process
// has. When the test process exits for any reason, including a panic outside the test
// goroutine, os.Exit, a test timeout or SIGKILL, the pipe closes and the watchdog kills the
// server.
type guard struct {
	cmd  *exec.Cmd
	pipe io.Closer
}

func protect(server *exec.Cmd) guard {
	w := exec.Command("/bin/sh", "-c", "read _; kill -9 "+strconv.Itoa(server.Process.Pid)+" 2>/dev/null")
	pipe, err := w.StdinPipe()
	if err != nil {
		return guard{}
	}
	if err := w.Start(); err != nil {
		pipe.Close()
		return guard{}
	}
	return guard{cmd: w, pipe: pipe}
}

// release stops the watchdog without letting it act, before the server is stopped, so it can
// never signal a reused pid.
func (g guard) release() {
	if g.cmd == nil {
		return
	}
	_ = g.cmd.Process.Kill()
	_ = g.cmd.Wait()
	_ = g.pipe.Close()
}
