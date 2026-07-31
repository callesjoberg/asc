using System;
using System.IO;
using System.Threading.Tasks;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;
using Windows.Storage;

namespace OcrHelper
{
    class Program
    {
        static async Task<int> Main(string[] args)
        {
            if (args.Length < 1)
            {
                Console.WriteLine("Error: Missing image path argument.");
                return 1;
            }

            string imagePath = Path.GetFullPath(args[0]);
            if (!File.Exists(imagePath))
            {
                Console.WriteLine($"Error: Image file does not exist at: {imagePath}");
                return 1;
            }

            try
            {
                StorageFile file = await StorageFile.GetFileFromPathAsync(imagePath);
                using (var stream = await file.OpenAsync(FileAccessMode.Read))
                {
                    BitmapDecoder decoder = await BitmapDecoder.CreateAsync(stream);
                    using (SoftwareBitmap softwareBitmap = await decoder.GetSoftwareBitmapAsync())
                    {
                        OcrEngine ocrEngine = OcrEngine.TryCreateFromUserProfileLanguages();
                        if (ocrEngine == null)
                        {
                            Console.WriteLine("Error: Could not create OcrEngine (no languages installed?).");
                            return 1;
                        }

                        OcrResult result = await ocrEngine.RecognizeAsync(softwareBitmap);
                        Console.WriteLine(result.Text);
                    }
                }
            }
            catch (Exception ex)
            {
                Console.WriteLine($"Error performing OCR: {ex.Message}");
                return 1;
            }

            return 0;
        }
    }
}
