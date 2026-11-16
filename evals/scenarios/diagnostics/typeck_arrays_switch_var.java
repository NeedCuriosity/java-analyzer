import java.util.List;
import java.util.Map;

class Main {
    static final int LIMIT = 10;

    static void log() {}

    void arrays(String s, int[] values, long offset, char c, byte b, Integer boxed, List<int[]> rows) {
        int[] a = {/*@a1*/"x"};
        String[] names = {/*@a2*/1, "y"};
        int first = values[/*@a3*/"0"];
        int second = values[/*@a4*/offset];
        int[] sized = new int[/*@a5*/2L];
        char ch = /*@a6*/s[0];
        byte[] ok1 = {1, 2, -128};
        long[] ok2 = {1, 2L, 'c'};
        Object[] ok3 = {1, "x", null};
        int[][] ok4 = {{1}, {2, 3}};
        int ok5 = values[c] + values[b] + values[boxed] + rows.get(0)[0];
        System.out.println("" + a + names + first + second + sized + ch + ok1 + ok2 + ok3 + ok4 + ok5);
    }

    void switches(int i, String s, char c, Integer boxed) {
        switch (i) {
            case /*@s1*/"a":
                break;
            case 'b':
            case LIMIT:
                break;
        }
        switch (s) {
            case /*@s2*/1:
                break;
            case "ok":
                break;
        }
        switch (c) {
            case 'a':
            case 66:
                break;
        }
        switch (boxed) {
            case 1:
                break;
        }
    }

    void vars(List<String> names) {
        var /*@v1*/nothing = null;
        var /*@v2*/array = {1, 2};
        var /*@v3*/fn = () -> 1;
        var /*@v4*/ref = String::length;
        var /*@v5*/result = log();
        var ok1 = 1;
        var ok2 = names;
        var ok3 = (Runnable) () -> {};
        var ok4 = new int[] {1};
        var ok5 = (String) null;
        for (var name : names) {
            System.out.println(name);
        }
        System.out.println("" + nothing + array + fn + ref + result + ok1 + ok2 + ok3 + ok4 + ok5);
    }

    void loops(String text, int count, Map<String, Integer> map, Iterable<String> items, String[] parts) {
        for (char ch : /*@f1*/text) {
            System.out.println(ch);
        }
        for (int n : /*@f2*/count) {
            System.out.println(n);
        }
        for (String key : map.keySet()) {
            System.out.println(key);
        }
        for (String item : items) {
            System.out.println(item);
        }
        for (String part : parts) {
            System.out.println(part);
        }
    }
}
//? diag type-mismatch a1
//? diag type-mismatch a2
//? diag type-mismatch a3
//? diag type-mismatch a4
//? diag type-mismatch a5
//? diag type-mismatch a6
//? diag type-mismatch s1
//? diag type-mismatch s2
//? diag type-mismatch v1
//? diag type-mismatch v2
//? diag type-mismatch v3
//? diag type-mismatch v4
//? diag type-mismatch v5
//? diag type-mismatch f1
//? diag type-mismatch f2
//? diag exact
