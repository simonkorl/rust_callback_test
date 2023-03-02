/// A dtp config refer to a set of dtp sending configuration
/// Such configurations are always saved in 'aitrans_block.txt' file
/// An exmaple:
/// | send_time_gap (s) | deadline (ms) | block_size (B) | priority (1, 2, 3)|
/// | -- | -- | -- | -- | 
/// |0.015025005 |  200 |   1235 |   1 |
/// | the gap between two adjasent sending | the deadline goal | the block size | the priority. Higher the number is, higher the priority is.| 

#ifndef CONFIG_PD_H
#define CONFIG_PD_H

#include <stdio.h>
#include <stdlib.h>
#include <ctype.h>
#include <string.h>
#include <sys/time.h>

__uint64_t getCurrentUsec();  //usec

struct dtp_config {
    int deadline;   // ms
    int priority;   //
    int block_size; // byte
    float send_time_gap;//s
};

typedef struct dtp_config dtp_config;

// return: config array, you have to release it 
// number: return the number of parsed dtp_config (MAX=10).
struct dtp_config* parse_dtp_config(const char *filename, int *number);

#endif // CONFIG_PD_H
