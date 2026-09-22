/* Frame and expression stress: many locals, large local arrays, deeply nested
 * blocks and long operator chains.  Exercises stack-slot allocation, alignment
 * and the depth bookkeeping of the expression stack machine.
 */
int printf(char *fmt, ...);

int big_local_array(void) {
    int data[512];
    int i;
    int sum;

    for (i = 0; i < 512; i++) {
        data[i] = (i * 7) % 23;
    }
    sum = 0;
    for (i = 0; i < 512; i++) {
        sum = sum + data[i];
    }
    return sum;
}

int char_grid(void) {
    char grid[32][32];
    int i;
    int j;
    int count;

    for (i = 0; i < 32; i++) {
        for (j = 0; j < 32; j++) {
            grid[i][j] = (char)((i + j) % 26 + 'a');
        }
    }
    count = 0;
    for (i = 0; i < 32; i++) {
        for (j = 0; j < 32; j++) {
            if (grid[i][j] == 'a') {
                count++;
            }
        }
    }
    printf("%c%c%c\n", grid[0][0], grid[1][1], grid[31][31]);
    return count;
}

int many_locals(void) {
    int a1 = 1;
    int a2 = 2;
    int a3 = 3;
    int a4 = 4;
    int a5 = 5;
    int a6 = 6;
    int a7 = 7;
    int a8 = 8;
    int a9 = 9;
    int a10 = 10;
    int a11 = 11;
    int a12 = 12;
    int a13 = 13;
    int a14 = 14;
    int a15 = 15;
    int a16 = 16;
    int a17 = 17;
    int a18 = 18;
    int a19 = 19;
    int a20 = 20;
    char c1 = 'a';
    char c2 = 'b';
    unsigned char u1 = 200;
    int *p1 = &a1;
    int *p2 = &a20;

    *p1 = *p1 + 100;
    *p2 = *p2 + 200;
    c1 = c1 + 1;
    c2 = c2 + 1;
    u1 = u1 + 10;

    return a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10
         + a11 + a12 + a13 + a14 + a15 + a16 + a17 + a18 + a19 + a20
         + c1 + c2 + u1;
}

int long_expression(void) {
    return 1 + 2 * 3 - 4 / 2 + 5 % 3 * 6 - 7 + 8 * 9 / 3
         - (10 - 11) * 12 + 13 + 14 * 15 - 16 / 4 + 17 - 18
         + (19 + 20) * (21 - 22) + 23 * 24 - 25 / 5 + 26 % 7
         + 27 - 28 + 29 * 30 / 6 - 31 + 32;
}

int deep_blocks(void) {
    int total = 0;
    {
        int a = 1;
        total += a;
        {
            int b = 2;
            total += b;
            {
                int c = 3;
                total += c;
                {
                    int d = 4;
                    total += d;
                    {
                        int e = 5;
                        total += e;
                        {
                            int f = 6;
                            total += f;
                            {
                                int g = 7;
                                total += g;
                            }
                        }
                    }
                }
            }
        }
    }
    return total;
}

int main(void) {
    int i;
    int total;

    printf("%d\n", big_local_array());
    printf("%d\n", char_grid());
    printf("%d\n", many_locals());
    printf("%d\n", long_expression());
    printf("%d\n", deep_blocks());

    /* repeated calls, to catch frame reuse mistakes */
    total = 0;
    for (i = 0; i < 20; i++) {
        total += big_local_array();
        total += deep_blocks();
    }
    printf("%d\n", total);
    return 0;
}
