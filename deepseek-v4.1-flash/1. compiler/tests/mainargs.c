int printf(char *fmt, ...);

int main(int argc, char **argv) {
    int i;
    printf("argc=%d\n", argc);
    for (i = 1; i < argc; i++) {
        printf("arg%d=%s\n", i, argv[i]);
    }
    if (argc > 1) {
        printf("first=%c\n", argv[1][0]);
        printf("last=%s\n", argv[argc - 1]);
    }
    return 0;
}
