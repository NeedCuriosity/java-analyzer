import java.util.ArrayList;
import java.util.List;
import java.util.function.Supplier;

class Main {
    List<String> names() {
        List<String> /*@returned*/result = new ArrayList<>();
        return result;
    }

    RuntimeException error(String message) {
        IllegalStateException /*@thrown*/e = new IllegalStateException(message);
        throw e;
    }

    int used(int a) {
        int sum = a + 1;
        System.out.println(sum);
        return sum;
    }

    int notNext(int a) {
        int sum = a + 1;
        a++;
        return sum;
    }

    int[] array() {
        int[] values = {1, 2};
        return values;
    }

    Supplier<String> lambda() {
        Supplier<String> s = () -> "x";
        return s;
    }

    @SuppressWarnings("unchecked")
    List<String> annotatedLocal(Object o) {
        @SuppressWarnings("unchecked") List<String> list = (List<String>) o;
        return list;
    }

    int commented(int a) {
        int b = a * 2;
        // 有注释时保留
        return b;
    }

    Object widened(int a) {
        long wide = a;
        return wide;
    }

    int fix(int a) {
        int /*@fix*/doubled = a * 2;
        return doubled;
    }
}
//? diag redundant-local returned
//? diag redundant-local thrown
//? diag redundant-local fix
//? diag only redundant-local
//? assist simplify fix ; has "    int fix(int a) {\n        return a * 2;\n    }"
