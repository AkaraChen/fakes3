package fakes3

import (
	"os/exec"
	"syscall"
)

// The kernel kills the server when the test process exits for any reason, including a panic
// outside the test goroutine, os.Exit, a test timeout or SIGKILL, when no cleanup runs.
func prepare(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Pdeathsig: syscall.SIGKILL}
}

type guard struct{}

func protect(*exec.Cmd) guard { return guard{} }

func (guard) release() {}
