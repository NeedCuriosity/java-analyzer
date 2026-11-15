//- /Stable.java
import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;

class Stable {
    private static final int[] PRIMES = {
        2, 3, 5,
        7, 11
    };

    interface Shape {
        double area();
    }

    void lambdas(List<String> names) {
        names.forEach(name -> {
            System.out.println(name);
        });
        Function<String, Integer> length = text -> {
            return text.length();
        };
        run(length,
                text -> {
                    return text;
                });
    }

    void run(Function<String, Integer> first, Function<String, String> second) {
    }

    Shape anonymous(double side) {
        return new Shape() {
            @Override
            public double area() {
                return side * side;
            }
        };
    }

    int control(int kind) {
        switch (kind) {
            case 1:
                return 10;
            case 2: {
                return 20;
            }
            default:
                break;
        }
        int result = switch (kind) {
            case 3 -> 30;
            case 4 -> {
                yield 40;
            }
            default -> 0;
        };
        try {
            result++;
        } catch (IllegalStateException | IllegalArgumentException e) {
            result--;
        } finally {
            result += 2;
        }
        do {
            result--;
        } while (result > 100);
        for (int i = 0; i < 3; i++) {
            result += i;
        }
        return result;
    }

    String text() {
        String joined = "a"
                + "b"
                + "c";
        List<String> list = new ArrayList<>(
                List.of(joined));
        return String.format("%s %s",
                joined,
                list);
    }
}
//- /Stable.java formatted
import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;

class Stable {
    private static final int[] PRIMES = {
        2, 3, 5,
        7, 11
    };

    interface Shape {
        double area();
    }

    void lambdas(List<String> names) {
        names.forEach(name -> {
            System.out.println(name);
        });
        Function<String, Integer> length = text -> {
            return text.length();
        };
        run(length,
                text -> {
                    return text;
                });
    }

    void run(Function<String, Integer> first, Function<String, String> second) {
    }

    Shape anonymous(double side) {
        return new Shape() {
            @Override
            public double area() {
                return side * side;
            }
        };
    }

    int control(int kind) {
        switch (kind) {
            case 1:
                return 10;
            case 2: {
                return 20;
            }
            default:
                break;
        }
        int result = switch (kind) {
            case 3 -> 30;
            case 4 -> {
                yield 40;
            }
            default -> 0;
        };
        try {
            result++;
        } catch (IllegalStateException | IllegalArgumentException e) {
            result--;
        } finally {
            result += 2;
        }
        do {
            result--;
        } while (result > 100);
        for (int i = 0; i < 3; i++) {
            result += i;
        }
        return result;
    }

    String text() {
        String joined = "a"
                + "b"
                + "c";
        List<String> list = new ArrayList<>(
                List.of(joined));
        return String.format("%s %s",
                joined,
                list);
    }
}
