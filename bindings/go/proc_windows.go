package fakes3

import (
	"os/exec"
	"sync"
	"syscall"
	"unsafe"
)

func prepare(*exec.Cmd) {}

// Servers are put in a job object that kills its processes when its last handle closes. This
// process holds the only handle, so when it exits for any reason, including a panic outside the
// test goroutine, os.Exit or a test timeout, Windows kills the servers.

var (
	kernel32                     = syscall.NewLazyDLL("kernel32.dll")
	procCreateJobObjectW         = kernel32.NewProc("CreateJobObjectW")
	procSetInformationJobObject  = kernel32.NewProc("SetInformationJobObject")
	procAssignProcessToJobObject = kernel32.NewProc("AssignProcessToJobObject")

	jobOnce sync.Once
	job     syscall.Handle
)

const (
	jobObjectExtendedLimitInformationClass = 9
	jobObjectLimitKillOnJobClose           = 0x2000
	processSetQuota                        = 0x0100
	processTerminate                       = 0x0001
)

type jobObjectBasicLimitInformation struct {
	PerProcessUserTimeLimit int64
	PerJobUserTimeLimit     int64
	LimitFlags              uint32
	MinimumWorkingSetSize   uintptr
	MaximumWorkingSetSize   uintptr
	ActiveProcessLimit      uint32
	Affinity                uintptr
	PriorityClass           uint32
	SchedulingClass         uint32
}

type ioCounters struct {
	ReadOperationCount, WriteOperationCount, OtherOperationCount uint64
	ReadTransferCount, WriteTransferCount, OtherTransferCount    uint64
}

type jobObjectExtendedLimitInformation struct {
	BasicLimitInformation jobObjectBasicLimitInformation
	IoInfo                ioCounters
	ProcessMemoryLimit    uintptr
	JobMemoryLimit        uintptr
	PeakProcessMemoryUsed uintptr
	PeakJobMemoryUsed     uintptr
}

func killOnExitJob() syscall.Handle {
	jobOnce.Do(func() {
		h, _, _ := procCreateJobObjectW.Call(0, 0)
		if h == 0 {
			return
		}
		var info jobObjectExtendedLimitInformation
		info.BasicLimitInformation.LimitFlags = jobObjectLimitKillOnJobClose
		ok, _, _ := procSetInformationJobObject.Call(h, jobObjectExtendedLimitInformationClass,
			uintptr(unsafe.Pointer(&info)), unsafe.Sizeof(info))
		if ok == 0 {
			syscall.CloseHandle(syscall.Handle(h))
			return
		}
		job = syscall.Handle(h)
	})
	return job
}

type guard struct{}

func protect(server *exec.Cmd) guard {
	j := killOnExitJob()
	if j == 0 {
		return guard{}
	}
	p, err := syscall.OpenProcess(processSetQuota|processTerminate, false, uint32(server.Process.Pid))
	if err != nil {
		return guard{}
	}
	defer syscall.CloseHandle(p)
	procAssignProcessToJobObject.Call(uintptr(j), uintptr(p))
	return guard{}
}

func (guard) release() {}
