// io_uring download spike: Linux only, no keep-alive, no directory listings.
//
// Build (liburing 2.9 or newer):
//   gcc -O2 -Wall -Wextra -o server server.c
//       -I/tmp/liburing/include -L/tmp/liburing/lib -luring
//       -Wl,-rpath,/tmp/liburing/lib
//
// Run:
//   ./server --root DIR --port 19530 --max 12000 --backlog 8192
//
// This intentionally uses read+send rather than sendfile: Linux io_uring has
// no IORING_OP_SENDFILE; a zero-copy variant would need splice through a pipe
// (two operations and two extra fds per connection).

#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <liburing.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

#define REQUEST_BUF 2048
#define HEADER_BUF 256
#define DATA_BUF 8192
#define ACCEPT_BATCH 64

#define UD_ACCEPT UINT64_MAX
#define OP_RECV 1
#define OP_SEND_HEADER 2
#define OP_READ_FILE 3
#define OP_SEND_BODY 4

enum state {
    ST_READING,
    ST_HEADER,
    ST_READ_FILE,
    ST_SEND_FILE,
};

struct conn {
    int fd;
    int file_fd;
    enum state state;
    char request[REQUEST_BUF];
    size_t request_len;
    char header[HEADER_BUF];
    size_t header_len;
    size_t header_pos;
    char data[DATA_BUF];
    size_t data_len;
    size_t data_pos;
    off_t file_offset;
    size_t remaining;
};

static struct conn *conns;
static size_t max_conns;
static int listener = -1;
static struct io_uring ring;

static void close_conn(size_t index) {
    struct conn *c = &conns[index];
    if (c->file_fd >= 0) {
        close(c->file_fd);
    }
    if (c->fd >= 0) {
        close(c->fd);
    }
    memset(c, 0, sizeof(*c));
    c->fd = -1;
    c->file_fd = -1;
}

static int submit_sqe(void) {
    if (io_uring_submit(&ring) < 0) {
        perror("io_uring_submit");
        return -1;
    }
    return 0;
}

static int queue_accept(void) {
    struct io_uring_sqe *sqe = io_uring_get_sqe(&ring);
    if (!sqe) {
        return -1;
    }
    io_uring_prep_accept(sqe, listener, NULL, NULL, 0);
    io_uring_sqe_set_data64(sqe, UD_ACCEPT);
    return submit_sqe();
}

static int queue_recv(size_t index) {
    struct conn *c = &conns[index];
    struct io_uring_sqe *sqe = io_uring_get_sqe(&ring);
    if (!sqe) {
        return -1;
    }
    io_uring_prep_recv(sqe, c->fd, c->request + c->request_len,
                       REQUEST_BUF - c->request_len, 0);
    io_uring_sqe_set_data64(sqe, ((uint64_t)index << 8) | OP_RECV);
    return submit_sqe();
}

static int queue_send_header(size_t index) {
    struct conn *c = &conns[index];
    struct io_uring_sqe *sqe = io_uring_get_sqe(&ring);
    if (!sqe) {
        return -1;
    }
    io_uring_prep_send(sqe, c->fd, c->header + c->header_pos,
                       c->header_len - c->header_pos, MSG_NOSIGNAL);
    io_uring_sqe_set_data64(sqe, ((uint64_t)index << 8) | OP_SEND_HEADER);
    return submit_sqe();
}

static int queue_read_file(size_t index) {
    struct conn *c = &conns[index];
    size_t want = c->remaining < DATA_BUF ? c->remaining : DATA_BUF;
    struct io_uring_sqe *sqe = io_uring_get_sqe(&ring);
    if (!sqe) {
        return -1;
    }
    io_uring_prep_read(sqe, c->file_fd, c->data, want, c->file_offset);
    io_uring_sqe_set_data64(sqe, ((uint64_t)index << 8) | OP_READ_FILE);
    return submit_sqe();
}

static int queue_send_body(size_t index) {
    struct conn *c = &conns[index];
    struct io_uring_sqe *sqe = io_uring_get_sqe(&ring);
    if (!sqe) {
        return -1;
    }
    io_uring_prep_send(sqe, c->fd, c->data + c->data_pos,
                       c->data_len - c->data_pos, MSG_NOSIGNAL);
    io_uring_sqe_set_data64(sqe, ((uint64_t)index << 8) | OP_SEND_BODY);
    return submit_sqe();
}

static int header_end(const char *buf, size_t len) {
    for (size_t i = 0; i + 3 < len; i++) {
        if (buf[i] == '\r' && buf[i + 1] == '\n' && buf[i + 2] == '\r' &&
            buf[i + 3] == '\n') {
            return (int)(i + 4);
        }
    }
    return -1;
}

static int parse_target(const char *request, size_t len, char *path,
                        size_t path_len) {
    size_t end = 0;
    while (end + 1 < len && !(request[end] == '\r' && request[end + 1] == '\n')) {
        end++;
    }
    if (end + 1 >= len) {
        return -1;
    }
    const char *line = request;
    const char *space = memchr(line, ' ', end);
    if (!space || (size_t)(space - line) < 3 ||
        memcmp(line, "GET", 3) != 0) {
        return -1;
    }
    const char *target = space + 1;
    const char *target_end = memchr(target, ' ', end - (size_t)(target - line));
    if (!target_end) {
        return -1;
    }
    if (*target != '/') {
        return -1;
    }
    target++;
    size_t n = (size_t)(target_end - target);
    while (n > 0 && target[n - 1] == '?') {
        n--;
    }
    for (size_t i = 0; i < n; i++) {
        if (target[i] == '?' || target[i] == '\0') {
            n = i;
            break;
        }
    }
    if (n == 0 || n >= path_len) {
        return -1;
    }
    memcpy(path, target, n);
    path[n] = '\0';
    return 0;
}

static int complete_request(size_t index) {
    struct conn *c = &conns[index];
    int end = header_end(c->request, c->request_len);
    if (end < 0) {
        return queue_recv(index);
    }
    char path[REQUEST_BUF];
    if (parse_target(c->request, (size_t)end, path, sizeof(path)) != 0) {
        return -1;
    }
    int file_fd = open(path, O_RDONLY | O_CLOEXEC);
    if (file_fd < 0) {
        return -1;
    }
    struct stat st;
    if (fstat(file_fd, &st) != 0 || !S_ISREG(st.st_mode)) {
        close(file_fd);
        return -1;
    }
    c->file_fd = file_fd;
    c->file_offset = 0;
    c->remaining = (size_t)st.st_size;
    c->header_len = (size_t)snprintf(
        c->header, sizeof(c->header),
        "HTTP/1.1 200 OK\r\nContent-Length: %llu\r\nConnection: close\r\n\r\n",
        (unsigned long long)st.st_size);
    if (c->header_len == 0 || c->header_len >= sizeof(c->header)) {
        return -1;
    }
    c->header_pos = 0;
    c->state = ST_HEADER;
    return queue_send_header(index);
}

static int handle_cqe(struct io_uring_cqe *cqe) {
    if (cqe->user_data == UD_ACCEPT) {
        int res = cqe->res;
        if (res < 0) {
            return -1;
        }
        size_t index;
        for (index = 0; index < max_conns; index++) {
            if (conns[index].fd < 0) {
                break;
            }
        }
        if (index == max_conns) {
            close(res);
            return 0;
        }
        struct conn *c = &conns[index];
        memset(c, 0, sizeof(*c));
        c->fd = res;
        c->file_fd = -1;
        c->state = ST_READING;
        return queue_recv(index);
    }

    size_t index = (size_t)(cqe->user_data >> 8);
    int op = (int)(cqe->user_data & 0xff);
    if (index >= max_conns || conns[index].fd < 0) {
        return 0;
    }
    struct conn *c = &conns[index];
    int res = cqe->res;
    if (res <= 0) {
        close_conn(index);
        return 0;
    }

    switch (op) {
    case OP_RECV:
        c->request_len += (size_t)res;
        if (c->request_len == sizeof(c->request)) {
            close_conn(index);
            return 0;
        }
        if (header_end(c->request, c->request_len) < 0) {
            return queue_recv(index);
        }
        if (complete_request(index) != 0) {
            close_conn(index);
        }
        return 0;
    case OP_SEND_HEADER:
        c->header_pos += (size_t)res;
        if (c->header_pos < c->header_len) {
            return queue_send_header(index);
        }
        c->state = ST_READ_FILE;
        return queue_read_file(index);
    case OP_READ_FILE:
        c->data_len = (size_t)res;
        c->data_pos = 0;
        c->state = ST_SEND_FILE;
        return queue_send_body(index);
    case OP_SEND_BODY:
        c->data_pos += (size_t)res;
        if (c->data_pos < c->data_len) {
            return queue_send_body(index);
        }
        c->file_offset += (off_t)c->data_len;
        c->remaining -= c->data_len;
        if (c->remaining == 0) {
            close_conn(index);
            return 0;
        }
        c->state = ST_READ_FILE;
        return queue_read_file(index);
    default:
        close_conn(index);
        return 0;
    }
}

int main(int argc, char **argv) {
    const char *root = NULL;
    int port = 19530;
    size_t max = 12000;
    int backlog = 8192;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--root") == 0 && i + 1 < argc) {
            root = argv[++i];
        } else if (strcmp(argv[i], "--port") == 0 && i + 1 < argc) {
            port = atoi(argv[++i]);
        } else if (strcmp(argv[i], "--max") == 0 && i + 1 < argc) {
            max = (size_t)strtoul(argv[++i], NULL, 10);
        } else if (strcmp(argv[i], "--backlog") == 0 && i + 1 < argc) {
            backlog = atoi(argv[++i]);
        } else {
            fprintf(stderr,
                    "usage: %s --root DIR [--port N] [--max N] [--backlog N]\n",
                    argv[0]);
            return 2;
        }
    }
    if (!root) {
        fprintf(stderr, "missing --root\n");
        return 2;
    }
    if (chdir(root) != 0) {
        perror("chdir");
        return 1;
    }
    conns = calloc(max, sizeof(*conns));
    if (!conns) {
        perror("calloc");
        return 1;
    }
    max_conns = max;
    for (size_t i = 0; i < max; i++) {
        conns[i].fd = -1;
        conns[i].file_fd = -1;
    }

    listener = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (listener < 0) {
        perror("socket");
        return 1;
    }
    int one = 1;
    setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    struct sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_port = htons((uint16_t)port),
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK),
    };
    if (bind(listener, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        perror("bind");
        return 1;
    }
    if (listen(listener, backlog) != 0) {
        perror("listen");
        return 1;
    }

    struct io_uring_params params;
    memset(&params, 0, sizeof(params));
    if (io_uring_queue_init(4096, &ring, 0) != 0) {
        perror("io_uring_queue_init");
        return 1;
    }
    for (int i = 0; i < ACCEPT_BATCH; i++) {
        if (queue_accept() != 0) {
            return 1;
        }
    }

    printf("io-uring: root=%s port=%d max=%zu backlog=%d conn_size=%zu\n",
           root, port, max, backlog, sizeof(struct conn));
    fflush(stdout);

    for (;;) {
        struct io_uring_cqe *cqe;
        int ret = io_uring_wait_cqe(&ring, &cqe);
        if (ret < 0) {
            if (ret == -EINTR) {
                continue;
            }
            fprintf(stderr, "wait_cqe: %s\n", strerror(-ret));
            return 1;
        }
        uint64_t user_data = cqe->user_data;
        struct io_uring_cqe copy = *cqe;
        io_uring_cqe_seen(&ring, cqe);
        if (handle_cqe(&copy) != 0) {
            return 1;
        }
        if (user_data == UD_ACCEPT) {
            queue_accept();
        }
    }
}
