class Main {
    static class InCtor {
        private final int size;
        private final int doubled;

        InCtor(int n) {
            doubled = /*@ctor_read*/size * 2;
            size = n;
        }
    }

    static class ThisQualified {
        private final String name;

        ThisQualified(String name) {
            System.out.println(this./*@this_read*/name);
            this.name = name;
        }
    }

    static class InInitializer {
        private final int a;

        {
            System.out.println(/*@init_read*/a);
        }

        InInitializer() {
            a = 1;
        }
    }

    static class InFieldInitializer {
        private final int a;
        private final int b = /*@field_init_read*/a + 1;

        InFieldInitializer() {
            a = 1;
        }
    }

    static class StaticRead {
        static final int LIMIT;

        static {
            System.out.println(/*@static_read*/LIMIT);
            LIMIT = 3;
        }
    }

    static class Compound {
        private final int count;

        Compound() {
            /*@compound*/count += 1;
        }
    }

    static class Ok1 {
        private final int a;
        private final int b;

        Ok1(int n) {
            a = n;
            b = a + 1;
        }
    }

    static class Ok2 {
        private final int a;

        Ok2(Ok2 other) {
            a = other.a + 1;
        }
    }

    static class Ok3 {
        private final int a;

        Ok3() {
            a = 1;
        }

        int twice() {
            Runnable printer = () -> System.out.println(this.a);
            printer.run();
            return a * 2;
        }
    }

    static class Ok4 {
        private final int a;

        Ok4(boolean flag) {
            if (flag) {
                a = 1;
            } else {
                a = 2;
            }
            System.out.println(a);
        }

        Ok4() {
            this(true);
            System.out.println(a);
        }
    }

    static class Ok5 {
        static final int X;
        static final int Y;

        static {
            X = 1;
            Y = X + 1;
        }
    }

    static class Ok6 {
        private final int a;

        {
            a = 1;
        }

        private final int b = a + 1;
    }

    static class Ok7 {
        private final int a;

        Ok7() {
            Object o = new Object() {
                int peek() {
                    return a;
                }
            };
            a = 1;
            System.out.println(o);
        }
    }
}
//? diag uninitialized-variable ctor_read
//? diag uninitialized-variable this_read
//? diag uninitialized-variable init_read
//? diag uninitialized-variable field_init_read
//? diag uninitialized-variable static_read
//? diag uninitialized-variable compound
//? diag exact
