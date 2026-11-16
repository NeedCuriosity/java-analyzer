import java.util.AbstractList;
import java.util.List;

abstract class Shape {
    abstract double area();
}

class Main {
    enum Color { RED, GREEN }

    void create() {
        Object a = new /*@n1*/Shape();
        Object b = new /*@n2*/List<String>();
        Object c = new /*@n3*/AbstractList<String>();
        Shape ok = new Shape() {
            double area() {
                return 1;
            }
        };
        System.out.println(a + "" + b + c + ok);
    }

    void jumps(int[] values) {
        /*@j1*/break;
    }

    void jumps2(int x) {
        if (x > 0) {
            /*@j2*/continue;
        }
        outer:
        for (int v = 0; v < x; v++) {
            for (int w = 0; w < v; w++) {
                if (w == 3) {
                    continue outer;
                }
                if (w == 4) {
                    break outer;
                }
            }
        }
        block:
        {
            if (x == 1) {
                break block;
            }
            /*@j3*/continue block;
        }
        switch (x) {
            case 1:
                break;
            default:
                break;
        }
        Runnable r = () -> {
            /*@j4*/break;
        };
        int y = switch (x) {
            case 1 -> {
                for (int i = 0; i < 2; i++) {
                    if (i == 1) {
                        break;
                    }
                }
                yield 1;
            }
            default -> 0;
        };
        System.out.println(y + "" + r);
    }

    void cases(int x, String s, Color c) {
        switch (x) {
            case 1:
            case /*@d1*/1:
                break;
            case 2, 3:
                break;
        }
        switch (s) {
            case "a" -> System.out.println();
            case /*@d2*/"a" -> System.out.println();
            default -> System.out.println();
        }
        switch (c) {
            case RED, GREEN, /*@d3*/RED -> System.out.println();
        }
    }
}
//? diag abstract-instantiation n1
//? diag abstract-instantiation n2
//? diag abstract-instantiation n3
//? diag only abstract-instantiation
//? diag misplaced-jump j1
//? diag misplaced-jump j2
//? diag misplaced-jump j3
//? diag misplaced-jump j4
//? diag only misplaced-jump
//? diag duplicate-case-label d1
//? diag duplicate-case-label d2
//? diag duplicate-case-label d3
//? diag only duplicate-case-label
