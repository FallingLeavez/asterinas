// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE
#include <sys/prctl.h>
#include <signal.h>
#include <stdlib.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>
#include <poll.h>

static volatile sig_atomic_t received_parent_death_signal;

void signal_handler(int signum)
{
	if (signum == SIGTERM) {
		received_parent_death_signal = 1;
	}
}

int main()
{
	int ready_pipe[2];
	int control_pipe[2];
	int result_pipe[2];
	if (pipe(ready_pipe) == -1 || pipe(control_pipe) == -1 ||
	    pipe(result_pipe) == -1) {
		perror("pipe");
		return EXIT_FAILURE;
	}

	pid_t pid = fork();

	if (pid == -1) {
		perror("fork");
		return EXIT_FAILURE;
	}

	if (pid > 0) {
		close(ready_pipe[1]);
		close(control_pipe[0]);
		close(result_pipe[1]);

		char ready;
		if (read(ready_pipe[0], &ready, sizeof(ready)) != sizeof(ready)) {
			fprintf(stderr, "child did not become ready\n");
			return EXIT_FAILURE;
		}
		close(ready_pipe[0]);

		if (write(control_pipe[1], "x", 1) != 1) {
			perror("write control pipe");
			return EXIT_FAILURE;
		}
		close(control_pipe[1]);

		int status;
		if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) ||
		    WEXITSTATUS(status) != EXIT_SUCCESS) {
			fprintf(stderr, "parent did not exit successfully\n");
			return EXIT_FAILURE;
		}

		struct pollfd result_pollfd = {
			.fd = result_pipe[0],
			.events = POLLIN,
		};
		if (poll(&result_pollfd, 1, 10000) != 1) {
			fprintf(stderr, "child did not receive parent-death signal\n");
			return EXIT_FAILURE;
		}
		char result;
		if (read(result_pipe[0], &result, sizeof(result)) != sizeof(result) ||
		    result != '1') {
			fprintf(stderr, "child reported an invalid result\n");
			return EXIT_FAILURE;
		}
		close(result_pipe[0]);
	} else {
		pid_t child_pid = fork();
		if (child_pid == -1) {
			perror("fork child");
			exit(EXIT_FAILURE);
		}

		if (child_pid > 0) {
			close(ready_pipe[0]);
			close(ready_pipe[1]);
			close(control_pipe[1]);
			close(result_pipe[0]);
			close(result_pipe[1]);

			char control;
			if (read(control_pipe[0], &control, sizeof(control)) !=
			    sizeof(control)) {
				exit(EXIT_FAILURE);
			}
			exit(EXIT_SUCCESS);
		}

		close(ready_pipe[0]);
		close(control_pipe[0]);
		close(control_pipe[1]);
		close(result_pipe[0]);

		sigset_t blocked_set;
		sigemptyset(&blocked_set);
		sigaddset(&blocked_set, SIGTERM);
		if (sigprocmask(SIG_BLOCK, &blocked_set, NULL) == -1) {
			perror("sigprocmask");
			exit(EXIT_FAILURE);
		}

		if (prctl(PR_SET_PDEATHSIG, SIGTERM) == -1) {
			perror("prctl");
			exit(EXIT_FAILURE);
		}

		struct sigaction sa;
		sa.sa_handler = signal_handler;
		sigemptyset(&sa.sa_mask);
		sa.sa_flags = 0;
		if (sigaction(SIGTERM, &sa, NULL) == -1) {
			perror("sigaction");
			exit(EXIT_FAILURE);
		}

		if (write(ready_pipe[1], "x", 1) != 1) {
			perror("write ready pipe");
			exit(EXIT_FAILURE);
		}
		close(ready_pipe[1]);

		sigset_t empty_set;
		sigemptyset(&empty_set);
		while (!received_parent_death_signal) {
			sigsuspend(&empty_set);
		}

		if (write(result_pipe[1], "1", 1) != 1) {
			perror("write result pipe");
			exit(EXIT_FAILURE);
		}
		close(result_pipe[1]);
		exit(EXIT_SUCCESS);
	}

	return EXIT_SUCCESS;
}
