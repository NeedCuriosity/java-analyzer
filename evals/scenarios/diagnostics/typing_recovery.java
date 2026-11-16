import java.util.ArrayList;
import java.util.List;

class Typing {
    private final List<String> names = new ArrayList<>();
    private int count;

    void missingBrace(String s) {
        for (String n : names) {
            count += n.length();
        names.add(s);
    }

    int afterBrokenMethod() {
        return count + names.size();
    }

    @Override
    public String toString() {
        return names.toString();
    }

    void lambdaInProgress() {
        names.forEach(n -> n./*@lambdaDot*/);
        count++;
    }

    void unclosedCall(String s) {
        names.add(
        count++;
        System.out.println(s);
    }

    void switchInProgress(int k) {
        switch (k) {
            case 1:
                count++;

    static int /*@helper*/helper() {
        return 1;
    }

    void ok() {
        count = /*@helperCall*/helper() + afterBrokenMethod();
        names.clear();
    }
}
//? complete lambdaDot has length isEmpty
//? def helperCall -> helper
//? diag only unresolved-method
//? diag only unresolved-name
//? diag only type-mismatch
//? diag only argument-mismatch
//? diag only unused-local
