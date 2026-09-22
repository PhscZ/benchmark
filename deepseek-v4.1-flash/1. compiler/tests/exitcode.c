/* Exit-status propagation: main's return value must become the process exit
 * code, including returns from inside nested blocks and loops.
 */
int printf(char *fmt, ...);
int getchar(void);

int status_from(int c) {
    if (c == 'a') {
        return 1;
    }
    if (c == 'b') {
        return 2;
    }
    if (c == 'c') {
        return 3;
    }
    return 0;
}

int main(void) {
    int c;

    c = getchar();
    if (c == -1) {
        /* No input: report a fixed status through a nested block. */
        {
            {
                return 7;
            }
        }
    }
    if (c == 'x') {
        int i;
        for (i = 0; i < 10; i++) {
            if (i == 4) {
                return 4;
            }
        }
    }
    printf("status=%d\n", status_from(c));
    return status_from(c);
}
