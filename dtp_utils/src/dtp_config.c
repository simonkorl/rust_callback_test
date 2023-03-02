#include "../include/dtp_config.h"
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <ctype.h>
#include <string.h>
#include <sys/time.h>

__uint64_t getCurrentUsec()  //usec
{
    struct timeval tv;
    gettimeofday(&tv, NULL);  //该函数在sys/time.h头文件中
    return tv.tv_sec * 1000*1000 + tv.tv_usec;
}

// return: config array, you have to release it
// number: return the number of parsed dtp_config (MAX=10).
struct dtp_config* parse_dtp_config(const char *filename,int *number)
{
    FILE *fd = NULL;

    float send_time_gap;
    int deadline;
    int priority;
    int block_size;

    int cfgs_len = 0;
    static int max_cfgs_len = 10000;
    dtp_config *cfgs = malloc(sizeof(*cfgs) * max_cfgs_len);

    fd = fopen(filename, "r");
    if (fd == NULL) {
        printf("fail to open config file in C code.\n");
        char buf[100];
        getcwd(buf, 100);
        printf("path: %s / %s \n", buf, filename);
        *number = 0;
        return NULL;
    }

    while (fscanf(fd, "%f %d %d %d", &send_time_gap, &deadline, &block_size, &priority) == 4 && cfgs_len < 10000) {
        cfgs[cfgs_len].send_time_gap = send_time_gap;
        cfgs[cfgs_len].deadline = deadline;
        cfgs[cfgs_len].block_size = block_size;
        cfgs[cfgs_len].priority = priority;

        cfgs_len ++;
    }
    *number = cfgs_len;
    fclose(fd);

    return cfgs;
}
