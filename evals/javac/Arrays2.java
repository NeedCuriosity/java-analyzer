import java.util.Arrays;
import java.util.List;

public class Arrays2 {
    public int sum(int[] values) {
        int total = 0;
        for (int i = 0; i < values.length; i++) {
            int value = values[i];
            total += value;
        }
        return total;
    }

    public int[][] grid(int rows, int cols) {
        int[][] cells = new int[rows][cols];
        for (int r = 0; r < rows; r++) {
            int[] row = cells[r];
            Arrays.fill(row, r);
        }
        return cells;
    }

    public String[] copy(String[] source) {
        String[] result = source.clone();
        String[] padded = Arrays.copyOf(result, result.length + 1);
        return padded;
    }

    public int varargs(String first, int... rest) {
        int count = rest.length;
        int head = count > 0 ? rest[0] : 0;
        return first.length() + head;
    }

    public int callVarargs() {
        int none = varargs("a");
        int some = varargs("b", 1, 2, 3);
        int[] packed = { 4, 5 };
        int array = varargs("c", packed);
        return none + some + array;
    }

    public List<String> asList(String[] values) {
        List<String> list = Arrays.asList(values);
        char[] chars = values[0].toCharArray();
        char first = chars[0];
        long wide = first;
        double ratio = wide / 2.0;
        System.out.println(ratio);
        return list;
    }

    public Object[] objects() {
        Object[] mixed = new Object[] { "a", 1, 2.0 };
        Object head = mixed[0];
        String text = (String) head;
        int length = text.length();
        mixed[1] = length;
        return mixed;
    }
}
