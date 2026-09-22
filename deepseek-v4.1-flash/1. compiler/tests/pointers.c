int printf(char *fmt, ...);

int table[5];
int primes[5] = {2, 3, 5, 7, 11};
char msg[] = "abc";
char *sp = "xyz";
int matrix[2][3] = {{1, 2, 3}, {4, 5, 6}};

/* Names that would be misread by the assembler if they were emitted verbatim:
 * registers, size directives, and assembler keywords. */
int ax;
int bp;
char byte;
int ptr;
int qword;
int offset;

int main(void) {
    int x;
    int y;
    int *p;
    int **pp;
    int i;

    x = 42;
    p = &x;
    printf("%d\n", *p);
    *p = 7;
    printf("%d\n", x);
    pp = &p;
    printf("%d\n", **pp);
    **pp = 9;
    printf("%d\n", x);

    // pointer arithmetic scales by the pointee size
    p = &x;
    printf("%d\n", p + 1 - p);
    y = 5;
    p = &x;
    printf("%d\n", *(p + 0));

    // arrays
    for (i = 0; i < 5; i++) {
        table[i] = i * i;
    }
    for (i = 0; i < 5; i++) {
        printf("%d ", table[i]);
    }
    printf("\n");

    printf("%d %d %d\n", primes[0], primes[2], primes[4]);
    printf("%d %d\n", *(primes + 1), *(1 + primes));
    printf("%d\n", 2 [primes]);

    // array-to-pointer conversion
    p = table;
    printf("%d %d\n", *p, p[3]);
    p = &table[2];
    printf("%d %d\n", *p, p[-1]);

    // pointer difference
    printf("%d %d\n", &table[4] - &table[1], &table[0] - &table[4]);

    // pointer comparison
    printf("%d %d\n", &table[1] < &table[2], &table[1] == &table[1]);

    // pointer increments
    p = table;
    p++;
    printf("%d\n", *p);
    p += 2;
    printf("%d\n", *p);
    p--;
    printf("%d\n", *p);
    --p;
    printf("%d\n", *p);
    printf("%d\n", *p++);
    printf("%d\n", *p);
    printf("%d\n", *--p);

    // char arrays and strings
    printf("%s\n", msg);
    printf("%s\n", sp);
    printf("%c%c%c\n", msg[0], msg[1], msg[2]);
    printf("%d\n", msg[3]);
    msg[0] = 'A';
    printf("%s\n", msg);
    sp = msg;
    printf("%s\n", sp);
    printf("%d\n", sp[0]);

    // pointer to pointer through an array of pointers
    {
        char *names[3];
        names[0] = "one";
        names[1] = "two";
        names[2] = "three";
        for (i = 0; i < 3; i++) {
            printf("%s ", names[i]);
        }
        printf("\n");
        printf("%c\n", *names[2]);
    }

    // multi-dimensional array
    printf("%d %d %d\n", matrix[0][0], matrix[1][2], matrix[1][0]);
    printf("%d\n", sizeof(matrix));
    printf("%d %d %d\n", sizeof(int), sizeof(char), sizeof(int *));

    // indexing through an int **
    {
        int *row[2];
        row[0] = matrix[0];
        row[1] = matrix[1];
        printf("%d %d\n", row[0][2], row[1][1]);
    }

    // address of an array element stored in a pointer, then written through
    {
        int *q = &table[3];
        *q = 100;
        printf("%d\n", table[3]);
    }

    // globals whose names collide with assembler registers/directives
    ax = 3;
    bp = 4;
    byte = 'Z';
    ptr = 5;
    qword = 6;
    offset = 7;
    printf("%d %d %c %d %d %d\n", ax, bp, byte, ptr, qword, offset);
    {
        int *p2 = &ax;
        int *p3 = &ptr;
        *p2 = 30;
        *p3 = 50;
        byte = byte + 1;
        printf("%d %d %c\n", ax, ptr, byte);
    }
    return 0;
}
