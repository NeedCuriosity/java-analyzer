import java.util.ArrayList;
import java.util.List;

class Main {
    List<String> names = new ArrayList<>();
    int count;

    void helper() {}

    static void calls() {
        List./*@s1*/size();
        Main./*@s2*/helper();
        int c = Main./*@s3*/count;
        Main.statics();
        System.out.println(c);
    }

    static void statics() {}

    void operands(String s, int[] a, Integer boxed) {
        /*@o1*/s++;
        boxed++;
        a[0]++;
        a./*@a1*/length();
        int len = a.length;
        String first = /*@e1*/a[0];
        String found = /*@e2*/names.stream().findFirst();
        String ok = names.get(0);
        System.out.println(len + first + found + ok);
    }
}
//? diag static-context s1
//? diag static-context s2
//? diag static-context s3
//? diag only static-context
//? diag bad-operand o1
//? diag only bad-operand
//? diag unresolved-method a1
//? diag only unresolved-method
//? diag type-mismatch e1
//? diag type-mismatch e2
//? diag only type-mismatch
