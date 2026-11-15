import java.io.PrintWriter;

class Main {
    boolean same(int[] a, int[] b, int[][] grid, int[][] other) {
        boolean x = /*@eq*/a.equals(b);
        return x && /*@deep*/grid.equals(other);
    }

    void print(int[] values, char[] chars, String[] names, PrintWriter out) {
        System.out.println(/*@println*/values);
        System.out.println(chars);
        String s = "values: " + /*@concat*/values;
        String t = /*@ts*/names.toString();
        String u = String.valueOf(/*@valueof*/names);
        String v = String.valueOf(chars);
        StringBuilder sb = new StringBuilder();
        sb.append(/*@append*/values);
        sb.append(chars);
        s += /*@plus_eq*/names;
        out.println(/*@writer*/values);
        int n = values.length;
        System.out.println(s + t + u + v + n + values.length);
    }
}
//? diag array-equals eq
//? diag array-equals deep
//? diag only array-equals
//? diag array-to-string println
//? diag array-to-string concat
//? diag array-to-string ts
//? diag array-to-string valueof
//? diag array-to-string append
//? diag array-to-string plus_eq
//? diag array-to-string writer
//? diag only array-to-string
//? assist fix_array_misuse eq ; has "boolean x = Arrays.equals(a, b);" ; has "import java.util.Arrays;"
//? assist fix_array_misuse deep ; has "Arrays.deepEquals(grid, other)"
//? assist fix_array_misuse concat ; has "\"values: \" + Arrays.toString(values)"
//? assist fix_array_misuse ts ; has "String t = Arrays.toString(names);"
