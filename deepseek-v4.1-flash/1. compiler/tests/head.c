/* Prints the first 3 lines of stdin, like `head -n 3`.  Exercises a global
 * char buffer, line-oriented input, char storage, and byte-exact output.
 */
int printf(char *fmt, ...);
int getchar(void);
int putchar(int c);

char buf[4096];
int len;

void read_line(void) {
    int c;
    len = 0;
    for (;;) {
        c = getchar();
        if (c == -1) {
            break;
        }
        if (len < 4095) {
            buf[len] = (char)c;
            len++;
        }
        if (c == '\n') {
            break;
        }
    }
}

int main(void) {
    int limit = 3;
    int n = 0;
    int i;

    while (n < limit) {
        read_line();
        if (len == 0) {
            break;
        }
        for (i = 0; i < len; i++) {
            putchar(buf[i]);
        }
        n++;
    }
    printf("lines=%d\n", n);
    return 0;
}
