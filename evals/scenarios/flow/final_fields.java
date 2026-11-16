class Main {
    static class NoCtor {
        private final int /*@no_ctor*/count;
    }

    static class MissingInCtor {
        private final String name;
        private final int size;

        MissingInCtor(String name) {
            this.name = name;
        /*@ctor_end*/}

        MissingInCtor() {
            this("x");
        }
    }

    static class Conditional {
        private final int value;

        Conditional(boolean flag) {
            if (flag) {
                value = 1;
            }
        /*@conditional*/}
    }

    static class StaticMissing {
        static final int /*@static_missing*/LIMIT;
    }

    static class Initialized {
        private final int x = 1;

        Initialized() {
            /*@has_init*/x = 2;
        }
    }

    static class Other {
        final int y;

        Other() {
            y = 0;
        }
    }

    static class Foreign {
        Foreign(Other other) {
            other./*@foreign*/y = 1;
        }
    }

    static class StaticInCtor {
        static final int COUNT;

        static {
            COUNT = 1;
        }

        StaticInCtor() {
            /*@static_in_ctor*/COUNT = 2;
        }
    }

    static class Ok1 {
        private final int a;
        private final int b = 2;
        private final int c;
        private static final int D;
        private final int e;

        static {
            D = 4;
        }

        {
            e = 5;
        }

        Ok1(int a) {
            this.a = a;
            c = 3;
        }

        Ok1() {
            this(1);
        }

        int sum() {
            return a + b + c + D + e;
        }
    }

    static class Ok2 {
        private final int value;

        Ok2(boolean flag) {
            if (flag) {
                value = 1;
            } else {
                value = 2;
            }
        }
    }

    static class Ok3 {
        private final int value;

        Ok3(int n) {
            if (n < 0) {
                throw new IllegalArgumentException();
            }
            value = n;
        }
    }

    static class Ok4 {
        private final int value;

        Ok4(String s) {
            try {
                value = Integer.parseInt(s);
            } catch (NumberFormatException e) {
                throw new IllegalStateException(e);
            }
        }
    }

    enum Ok5 {
        A(1), B(2);

        private final int code;

        Ok5(int code) {
            this.code = code;
        }
    }

    static class Ok6 {
        private final int value;

        Ok6(int n) {
            switch (n) {
                case 1 -> value = 1;
                default -> value = 2;
            }
        }
    }

    static class Ok7 {
        private final Runnable task;
        private final int n;

        Ok7(int n) {
            this.n = n;
            task = () -> System.out.println(this.n);
        }
    }

    static class Ok8 {
        final int x;

        Ok8() {
            label: {
                x = 1;
                break label;
            }
        }
    }

    record Ok9(int x) {
        Ok9 {
            if (x < 0) {
                throw new IllegalArgumentException();
            }
        }
    }
}
//? diag uninitialized-final-field no_ctor
//? diag uninitialized-final-field ctor_end
//? diag uninitialized-final-field conditional
//? diag uninitialized-final-field static_missing
//? diag assign-to-final has_init
//? diag assign-to-final foreign
//? diag assign-to-final static_in_ctor
//? diag exact
