int printf(char *fmt, ...);

char gc = 'A';
unsigned char guc = 200;
int gi = 1000;

int takes_char(char c) {
    return c;
}

int takes_uchar(unsigned char c) {
    return c;
}

int takes_int(int v) {
    return v;
}

int main(void) {
    char c;
    unsigned char uc;
    int i;

    printf("%d %d %d\n", sizeof(char), sizeof(unsigned char), sizeof(int));
    printf("%d %d %d\n", sizeof(gc), sizeof(guc), sizeof(gi));

    c = 65;
    printf("%d %c\n", c, c);
    c = 200;
    printf("%d\n", c);
    uc = 200;
    printf("%d\n", uc);
    uc = 65;
    printf("%d %c\n", uc, uc);

    // char is signed, unsigned char is not
    printf("%d %d\n", (char)200, (unsigned char)200);
    printf("%d %d\n", (char)-1, (unsigned char)-1);

    // promotions to int
    c = 100;
    uc = 100;
    printf("%d %d\n", c + c, uc + uc);
    c = 100;
    printf("%d\n", c * 2);
    uc = 200;
    printf("%d\n", uc + uc);

    // casts
    i = 300;
    printf("%d %d\n", (char)i, (unsigned char)i);
    printf("%d\n", (int)(char)i);
    i = -1;
    printf("%d %d\n", (unsigned char)i, (char)i);

    // global initialization from constants
    printf("%d %d %d\n", gc, guc, gi);

    // arguments narrower than int
    printf("%d %d\n", takes_char(65), takes_uchar(200));
    printf("%d %d\n", takes_char(-1), takes_int(-1));

    // char arithmetic wrapping at 8 bits on store
    c = 127;
    c = c + 1;
    printf("%d\n", c);
    uc = 255;
    uc = uc + 1;
    printf("%d\n", uc);

    // char comparison promotes to int
    c = -1;
    uc = 1;
    printf("%d\n", c < uc);

    // casts to and from pointers
    {
        int *p = &i;
        printf("%d\n", *(int *)p);
    }

    // void return type used in an expression statement
    {
        int r = takes_int(7);
        printf("%d\n", r);
    }

    // integer/pointer round trip
    {
        char buf[4];
        int *q;
        buf[0] = 1;
        buf[1] = 2;
        q = (int *)buf;
        printf("%d\n", sizeof(q) == 8);
    }
    return 0;
}
