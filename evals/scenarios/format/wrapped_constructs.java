//- /Wrapped.java
import java.util.List;

class Wrapped {
    private static final int[] PRIMES = {2, 3, 5};

    List<String> chained(List<String> names) {
        return names.stream()
                .filter(name -> !name.isEmpty())
                .map(String::trim)
                .toList();
    }

    boolean wrappedCondition(int a, int b) {
        if (a > b
                && b > 0
                && a < 100) {
            return true;
        } else if (a == b) {
            return false;
        }
        while (a > 0
                && b > 0) {
            a--;
        }
        return a + b
                + PRIMES.length > 3;
    }

    int wrappedControl(int result) {
        try {
            result++;
        } catch (IllegalStateException
                | IllegalArgumentException e) {
            result--;
        } finally {
            result += 2;
        }
        for (int i = 0;
                i < 3;
                i++) {
            result += i;
        }
        return result;
    }
}
//- /Wrapped.java formatted
import java.util.List;

class Wrapped {
    private static final int[] PRIMES = {2, 3, 5};

    List<String> chained(List<String> names) {
        return names.stream()
                .filter(name -> !name.isEmpty())
                .map(String::trim)
                .toList();
    }

    boolean wrappedCondition(int a, int b) {
        if (a > b
                && b > 0
                && a < 100) {
            return true;
        } else if (a == b) {
            return false;
        }
        while (a > 0
                && b > 0) {
            a--;
        }
        return a + b
                + PRIMES.length > 3;
    }

    int wrappedControl(int result) {
        try {
            result++;
        } catch (IllegalStateException
                | IllegalArgumentException e) {
            result--;
        } finally {
            result += 2;
        }
        for (int i = 0;
                i < 3;
                i++) {
            result += i;
        }
        return result;
    }
}
